//! PDF JPEG 2000 adaptation: 7.4.9 and Table 87.
mod channels;
#[cfg(test)]
mod tests;

use super::Pixels;
use crate::image::Association;
use crate::{ImageAlpha, ImageKind, Precision, Samples, features, parameters};
use pagura_core::convert::{u32_to_usize, usize_to_u64};
use pagura_core::{Diagnostic, Diagnostics, Dictionary, Error, Limits, Position, Result, Value};
use pagura_imagecodec::jpx::{self, EnumeratedColorSpace as Space};

/// `/SMaskInData` (Table 87): what an opacity channel in the data means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SMaskInData {
    /// 0: ignore it.
    Ignore,
    /// 1: a separate soft mask.
    Separate,
    /// 2: a soft mask already multiplied into the color samples.
    Premultiplied,
}

impl SMaskInData {
    fn read(dictionary: &Dictionary) -> Result<Self> {
        let mode = match parameters::integer(dictionary, b"SMaskInData", 0)? {
            0 => Self::Ignore,
            1 => Self::Separate,
            2 => Self::Premultiplied,
            _ => return Err(Error::syntax("invalid SMaskInData or simultaneous SMask")),
        };
        if mode != Self::Ignore && dictionary.get(b"SMask").is_some_and(|v| !v.is_null()) {
            return Err(Error::syntax("invalid SMaskInData or simultaneous SMask"));
        }
        Ok(mode)
    }

    const fn opacity(self) -> bool {
        !matches!(self, Self::Ignore)
    }
}

/// The image a dictionary declares: its size and, when it has a
/// `/ColorSpace`, that space's component count.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Declared {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) components: Option<u8>,
    /// Whether the space is `/Indexed`, which interprets the stored samples
    /// itself: a container palette would expand them a second time (7.4.9).
    pub(crate) indexed: bool,
}

/// A decoded JPEG 2000 image in PDF terms.
#[derive(Debug)]
pub(crate) struct Adapted {
    pub(crate) pixels: Pixels,
    /// The opacity plane, when `/SMaskInData` asks for it.
    pub(crate) alpha: Option<ImageAlpha>,
    /// The container's color, when the dictionary has no `/ColorSpace`.
    pub(crate) kind: Option<ImageKind>,
}

pub(crate) fn decode(
    data: &[u8],
    dictionary: &Dictionary,
    declared: Declared,
    limits: &Limits,
    diagnostics: &mut Diagnostics,
) -> Result<Adapted> {
    let mode = SMaskInData::read(dictionary)?;
    if jpx::is_codestream(data) {
        diagnostics.repair(
            Diagnostic::new(
                crate::repairs::JPX_CODESTREAM,
                "bare JPEG 2000 codestream decoded without a JP2 container (7.4.9)",
            )
            .at(Position::Buffer(0)),
        )?;
    }
    if declared.indexed && mode.opacity() {
        return Err(Error::unsupported(
            features::JPX_SAMPLE_LAYOUT,
            "Indexed JPX opacity requires channel mapping",
        ));
    }
    let mut options = jpx::Options::default();
    options.ignore_color = declared.components.is_some();
    options.ignore_opacity = !mode.opacity();
    options.ignore_palette = declared.indexed;
    let mut decoded = jpx::decode_with_diagnostics(data, limits, &options, diagnostics)?;
    if !mode.opacity() {
        channels::discard_opacity(&mut decoded, limits)?;
    }
    adapt(decoded, mode, declared, limits)
}

pub(crate) fn adapt(
    placement: jpx::Decoded,
    mode: SMaskInData,
    declared: Declared,
    limits: &Limits,
) -> Result<Adapted> {
    let jpx::Decoded {
        image: decoded,
        reference_origin,
        component_geometry,
        ..
    } = placement;
    check_layout(
        &decoded,
        &component_geometry,
        component_geometry.capacity(),
        mode,
        declared,
        limits,
    )?;
    let mut components = Vec::with_capacity(decoded.components.len());
    components.extend(
        decoded
            .components
            .iter()
            .zip(&component_geometry)
            .enumerate()
            .filter(|(index, _)| Some(*index) != decoded.alpha)
            .map(|(_, pair)| pair),
    );
    let first = components
        .first()
        .ok_or(Error::syntax("JPX has no color components"))?;
    let count =
        u8::try_from(components.len()).map_err(|_| Error::limit("too many JPX components"))?;
    if declared.components.is_some_and(|n| n != count) {
        return Err(Error::syntax(
            "JPX component count disagrees with color space",
        ));
    }
    let pixels = parameters::budget(declared.width, declared.height, 1, 1, limits)?;
    validate_planes(&decoded, &component_geometry, reference_origin, mode)?;
    let mixed = components.iter().any(|c| c.0.bits != first.0.bits);
    let precision = if mixed { 16 } else { first.0.bits };
    let values = parameters::budget(declared.width, declared.height, count, precision, limits)?;
    let storage = storage_bytes(
        &decoded,
        component_geometry.capacity(),
        values,
        precision,
        mode,
    )?;
    if storage > limits.decoded_bytes {
        return Err(Error::limit("JPX adapter sample budget exceeded"));
    }
    let samples = interleave(&components, values, declared.width, reference_origin)?;
    let precision = if mixed {
        Precision::Mixed(components.iter().map(|c| c.0.bits).collect())
    } else {
        Precision::Uniform(first.0.bits)
    };
    let alpha = if mode.opacity() {
        Some(opacity_plane(
            &decoded,
            &component_geometry,
            mode,
            declared.width,
            reference_origin,
            pixels,
        )?)
    } else {
        None
    };
    let kind = if declared.components.is_none() {
        Some(embedded_space(decoded.color, count)?)
    } else {
        None
    };
    Ok(Adapted {
        pixels: Pixels {
            components: count,
            precision,
            samples,
        },
        alpha,
        kind,
    })
}

/// Checks the decoded layout against the dictionary before any sample is
/// copied: geometry, dimensions, the opacity channel and the storage of the
/// component lists, whose retained capacity counts too.
fn check_layout(
    decoded: &jpx::JpxImage,
    geometry: &[jpx::ComponentGeometry],
    geometry_capacity: usize,
    mode: SMaskInData,
    declared: Declared,
    limits: &Limits,
) -> Result<()> {
    if geometry.len() != decoded.components.len() {
        return Err(Error::syntax("invalid JPX component geometry count"));
    }
    if decoded.width != declared.width || decoded.height != declared.height {
        return Err(Error::syntax(
            "JPX dimensions disagree with image dictionary",
        ));
    }
    if decoded
        .alpha
        .is_some_and(|index| index >= decoded.components.len())
    {
        return Err(Error::syntax("invalid JPX alpha component index"));
    }
    if mode.opacity() && decoded.alpha.is_none() {
        return Err(Error::syntax("SMaskInData requires an opacity channel"));
    }
    if storage_bytes(decoded, geometry_capacity, 0, 8, SMaskInData::Ignore)? > limits.decoded_bytes
    {
        return Err(Error::limit("JPX component adapter budget exceeded"));
    }
    Ok(())
}

/// The opacity channel as a plane with one sample per image pixel.
fn opacity_plane(
    decoded: &jpx::JpxImage,
    geometry: &[jpx::ComponentGeometry],
    mode: SMaskInData,
    width: u32,
    reference_origin: [u32; 2],
    pixels: usize,
) -> Result<ImageAlpha> {
    let opacity = decoded
        .alpha
        .and_then(|index| decoded.components.get(index).zip(geometry.get(index)))
        .ok_or(Error::syntax("missing JPX opacity channel"))?;
    Ok(ImageAlpha::new(
        opacity.0.bits,
        interleave(&[opacity], pixels, width, reference_origin)?,
        if mode == SMaskInData::Premultiplied {
            Association::Premultiplied
        } else {
            Association::Straight
        },
    ))
}

fn validate_planes(
    decoded: &jpx::JpxImage,
    geometries: &[jpx::ComponentGeometry],
    reference_origin: [u32; 2],
    mode: SMaskInData,
) -> Result<()> {
    for (index, (component, geometry)) in decoded.components.iter().zip(geometries).enumerate() {
        if mode.opacity() || Some(index) != decoded.alpha {
            validate_component(
                component,
                geometry,
                decoded.width,
                decoded.height,
                reference_origin,
            )?;
        }
    }
    Ok(())
}

fn validate_component(
    c: &jpx::JpxComponent,
    geometry: &jpx::ComponentGeometry,
    width: u32,
    height: u32,
    reference_origin: [u32; 2],
) -> Result<()> {
    if !(1..=16).contains(&c.bits) {
        return Err(Error::unsupported(
            features::JPX_SAMPLE_LAYOUT,
            "JPX precision exceeds sample representation",
        ));
    }
    for (axis, extent, samples) in [(0, width, c.width), (1, height, c.height)] {
        let step = geometry.step[axis];
        if step == 0 || samples == 0 {
            return Err(Error::syntax("invalid JPX sampling step"));
        }
        let start = u64::from(reference_origin[axis]).div_ceil(u64::from(step));
        let end = (u64::from(reference_origin[axis]) + u64::from(extent)).div_ceil(u64::from(step));
        if start * u64::from(step) != u64::from(geometry.origin[axis])
            || end - start != u64::from(samples)
        {
            return Err(Error::syntax("inconsistent JPX component placement"));
        }
    }
    if u64::try_from(c.samples.len()).ok() != Some(u64::from(c.width) * u64::from(c.height)) {
        return Err(Error::syntax("invalid JPX sample count"));
    }
    let max = u16::MAX >> (16 - c.bits);
    if c.samples.iter().any(|&v| v > max) {
        return Err(Error::syntax("JPX sample exceeds precision"));
    }
    Ok(())
}

fn storage_bytes(
    decoded: &jpx::JpxImage,
    geometry_capacity: usize,
    values: usize,
    bits: u8,
    mode: SMaskInData,
) -> Result<usize> {
    let add = |a: usize, b: usize| a.checked_add(b).ok_or(Error::limit("JPX storage overflow"));
    let mut bytes = 0usize;
    for c in &decoded.components {
        bytes = add(
            bytes,
            c.samples
                .len()
                .checked_mul(2)
                .ok_or(Error::limit("JPX storage overflow"))?,
        )?;
    }
    bytes = add(
        bytes,
        values
            .checked_mul(if bits <= 8 { 1 } else { 2 })
            .ok_or(Error::limit("JPX storage overflow"))?,
    )?;
    if mode.opacity()
        && let Some(alpha) = decoded.alpha.and_then(|i| decoded.components.get(i))
    {
        bytes = add(
            bytes,
            usize::try_from(u64::from(decoded.width) * u64::from(decoded.height))
                .map_err(|_| Error::limit("JPX opacity size overflow"))?
                .checked_mul(if alpha.bits <= 8 { 1 } else { 2 })
                .ok_or(Error::limit("JPX alpha storage overflow"))?,
        )?;
    }
    if let jpx::JpxColor::Icc(profile) = &decoded.color {
        bytes = add(bytes, profile.len())?;
    }
    // Ignoring opacity drops its samples, but truncating the component lists
    // retains their backing allocations. Charge their capacities until drop.
    let component_bytes = decoded
        .components
        .capacity()
        .checked_mul(size_of::<jpx::JpxComponent>())
        .ok_or(Error::limit("JPX component storage overflow"))?;
    let geometry_bytes = geometry_capacity
        .checked_mul(size_of::<jpx::ComponentGeometry>())
        .ok_or(Error::limit("JPX geometry storage overflow"))?;
    let reference_bytes = decoded
        .components
        .len()
        .checked_mul(2 * size_of::<&jpx::JpxComponent>() + 1)
        .ok_or(Error::limit("JPX component storage overflow"))?;
    add(
        add(add(bytes, component_bytes)?, geometry_bytes)?,
        reference_bytes,
    )
}

/// Interleaves `count` values of `components`, one byte each when they share
/// one precision of at most 8 bits, else two.
fn interleave(
    components: &[(&jpx::JpxComponent, &jpx::ComponentGeometry)],
    count: usize,
    width: u32,
    reference_origin: [u32; 2],
) -> Result<Samples> {
    let first = components
        .first()
        .ok_or(Error::syntax("missing JPX components"))?
        .0;
    let pixels = count / components.len();
    let sample = |pixel: usize, c: &jpx::JpxComponent, g: &jpx::ComponentGeometry| -> Result<u16> {
        // T.800 B.2/G.4 fix the sample positions. Piecewise-constant expansion
        // with border replication is the chosen resampling policy; no phase
        // reset occurs at a nonzero reference-grid origin.
        let columns = u32_to_usize(width);
        let x = u64::from(reference_origin[0]) + usize_to_u64(pixel % columns);
        let y = u64::from(reference_origin[1]) + usize_to_u64(pixel / columns);
        let x = (x.saturating_sub(u64::from(g.origin[0])) / u64::from(g.step[0]))
            .min(u64::from(c.width - 1));
        let y = (y.saturating_sub(u64::from(g.origin[1])) / u64::from(g.step[1]))
            .min(u64::from(c.height - 1));
        let index = usize::try_from(y * u64::from(c.width) + x)
            .map_err(|_| Error::limit("JPX sample index overflow"))?;
        c.samples
            .get(index)
            .copied()
            .ok_or(Error::syntax("invalid JPX sample index"))
    };
    if first.bits <= 8 && components.iter().all(|c| c.0.bits == first.bits) {
        let mut values = Vec::with_capacity(count);
        for i in 0..pixels {
            for &(c, g) in components {
                values.push(
                    u8::try_from(sample(i, c, g)?)
                        .map_err(|_| Error::syntax("JPX sample exceeds byte precision"))?,
                );
            }
        }
        return Ok(Samples::Bytes(values));
    }
    let mut values = Vec::with_capacity(count);
    for i in 0..pixels {
        for &(c, g) in components {
            values.push(sample(i, c, g)?);
        }
    }
    Ok(Samples::Words(values))
}

fn embedded_space(color: jpx::JpxColor, count: u8) -> Result<ImageKind> {
    let space = match (&color, count) {
        (jpx::JpxColor::Enumerated(Space::GRAYSCALE), 1) => Some(b"DeviceGray".as_slice()),
        (jpx::JpxColor::Enumerated(Space::SRGB), 3) => Some(b"DeviceRGB".as_slice()),
        (jpx::JpxColor::Enumerated(Space::CMYK), 4) => Some(b"DeviceCMYK".as_slice()),
        (jpx::JpxColor::Enumerated(Space::CMYK | Space::SRGB | Space::GRAYSCALE), _) => {
            return Err(Error::syntax(
                "JPX colour space disagrees with channel count",
            ));
        }
        (jpx::JpxColor::Enumerated(Space::CIEJAB), _) => {
            return Err(Error::syntax("CIEJab is forbidden for PDF JPX images"));
        }
        (jpx::JpxColor::Enumerated(Space::SYCC), 3) | (jpx::JpxColor::Icc(_), _) => None,
        (jpx::JpxColor::Enumerated(Space::SYCC), _) => {
            return Err(Error::syntax("JPX sYCC requires three colour channels"));
        }
        (jpx::JpxColor::Enumerated(_), _) => {
            return Err(Error::unsupported(
                features::JPX_COLOR,
                "JPX enumerated colour space is not implemented",
            ));
        }
        (jpx::JpxColor::Unspecified, _) => {
            return Err(Error::syntax(
                "JPX without dictionary ColorSpace requires container colour information",
            ));
        }
        _ => {
            return Err(Error::unsupported(
                features::JPX_COLOR,
                "JPX colour description is not implemented",
            ));
        }
    };
    Ok(space.map_or_else(
        || ImageKind::EmbeddedColor { color },
        |space| ImageKind::Color {
            color_space: Value::name(space),
        },
    ))
}
