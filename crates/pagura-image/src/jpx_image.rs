//! PDF JPEG 2000 adaptation: ISO 32000-2 7.4.9 and Table 87.
use crate::{DecodedImage, ImageAlpha, ImageKind, MaskKind, Samples, features, jpx, parameters};
use pagura_core::{Dictionary, Error, Limits, Recovery, Result, Value};

/// How a PDF JPXDecode image is decoded: its `SMaskInData` mode and the
/// container metadata it ignores (7.4.9, Table 87).
#[derive(Debug, Clone)]
pub(crate) struct Jpx {
    pub(crate) mode: i64,
    options: jpx::Options,
    indexed: bool,
}

/// A JPXDecode image's colour channels as decoding will return them,
/// without its samples.
#[derive(Debug, Clone)]
pub(crate) struct Description {
    /// Precision of each colour channel, in interleave order.
    pub(crate) bits: Vec<u8>,
    /// The sample interpretation of an image without a dictionary colour
    /// space, from the container.
    pub(crate) kind: Option<ImageKind>,
}

impl Jpx {
    /// Validates the dictionary entries and data framing that decoding
    /// checks before it reads the container.
    pub(crate) fn read(data: &[u8], dictionary: &Dictionary, kind: &ImageKind) -> Result<Self> {
        let mode = parameters::integer(dictionary, b"SMaskInData", 0)?;
        if !(0..=2).contains(&mode)
            || (mode != 0 && dictionary.get(b"SMask").is_some_and(|v| !v.is_null()))
        {
            return Err(Error::syntax("invalid SMaskInData or simultaneous SMask"));
        }
        if data.starts_with(&[255, 79]) {
            return Err(Error::syntax(
                "PDF JPXDecode requires a full JPEG 2000 container",
            ));
        }
        let options = jpx::Options {
            ignore_color: !matches!(kind, ImageKind::Color { color_space: None }),
            ignore_opacity: mode == 0,
        };
        let indexed = matches!(kind, ImageKind::Color { color_space: Some(Value::Array(space)) }
            if space.first().and_then(Value::as_name).is_some_and(|name| name == b"Indexed"));
        if indexed && mode != 0 {
            return Err(Error::unsupported(
                features::JPX_SAMPLE_LAYOUT,
                "Indexed JPX opacity requires channel mapping",
            ));
        }
        Ok(Self {
            mode,
            options,
            indexed,
        })
    }

    pub(crate) fn decode(
        &self,
        data: &[u8],
        image: &mut DecodedImage,
        limits: &Limits,
        diagnostics: &mut pagura_core::Diagnostics,
    ) -> Result<()> {
        let mut decoded = jpx::decode_pdf(data, limits, &self.options, diagnostics, self.indexed)?;
        if self.mode == 0 {
            super::jpx_channels::discard_opacity(data, &mut decoded, limits)?;
        }
        adapt(decoded, self.mode, image, limits)
    }

    /// The colour channels of `data` for an image of `width` × `height`
    /// whose colour space has `components` (0 when the dictionary has
    /// none), with the checks of decoding that need no samples.
    pub(crate) fn describe(
        &self,
        data: &[u8],
        (width, height, components): (u32, u32, u8),
        limits: &Limits,
        mode: Recovery,
    ) -> Result<Description> {
        let mut decoded = jpx::describe_pdf(data, limits, &self.options, mode, self.indexed)?;
        if self.mode == 0 {
            super::jpx_channels::discard_opacity(data, &mut decoded, limits)?;
        }
        let jpx::Decoded {
            image: decoded,
            reference_origin,
            component_geometry,
            ..
        } = decoded;
        if component_geometry.len() != decoded.components.len() {
            return Err(Error::syntax("invalid JPX component geometry count"));
        }
        if decoded.width != width || decoded.height != height {
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
        if self.mode != 0 && decoded.alpha.is_none() {
            return Err(Error::syntax("SMaskInData requires an opacity channel"));
        }
        let colours: Vec<_> = decoded
            .components
            .iter()
            .zip(&component_geometry)
            .enumerate()
            .filter(|(index, _)| Some(*index) != decoded.alpha)
            .map(|(_, pair)| pair)
            .collect();
        let first = colours
            .first()
            .ok_or(Error::syntax("JPX has no colour components"))?;
        let count =
            u8::try_from(colours.len()).map_err(|_| Error::limit("too many JPX components"))?;
        if components != 0 && components != count {
            return Err(Error::syntax(
                "JPX component count disagrees with colour space",
            ));
        }
        parameters::budget(width, height, 1, 1, limits)?;
        for (index, (component, geometry)) in decoded
            .components
            .iter()
            .zip(&component_geometry)
            .enumerate()
        {
            if self.mode != 0 || Some(index) != decoded.alpha {
                validate_placement(component, geometry, (width, height), reference_origin)?;
            }
        }
        let mixed = colours.iter().any(|c| c.0.bits != first.0.bits);
        let precision = if mixed { 16 } else { first.0.bits };
        parameters::budget(width, height, count, precision, limits)?;
        let kind = (components == 0)
            .then(|| embedded_space(decoded.color, count))
            .transpose()?;
        Ok(Description {
            bits: colours.iter().map(|c| c.0.bits).collect(),
            kind,
        })
    }
}

pub(crate) fn adapt(
    placement: jpx::Decoded,
    mode: i64,
    image: &mut DecodedImage,
    limits: &Limits,
) -> Result<()> {
    let jpx::Decoded {
        image: decoded,
        reference_origin,
        component_geometry,
        ..
    } = placement;
    if component_geometry.len() != decoded.components.len() {
        return Err(Error::syntax("invalid JPX component geometry count"));
    }
    if decoded.width != image.width || decoded.height != image.height {
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
    if mode != 0 && decoded.alpha.is_none() {
        return Err(Error::syntax("SMaskInData requires an opacity channel"));
    }
    if storage_bytes(&decoded, component_geometry.capacity(), 0, 8, 0)? > limits.decoded_bytes {
        return Err(Error::limit("JPX component adapter budget exceeded"));
    }
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
        .ok_or(Error::syntax("JPX has no colour components"))?;
    let count =
        u8::try_from(components.len()).map_err(|_| Error::limit("too many JPX components"))?;
    if image.components != 0 && image.components != count {
        return Err(Error::syntax(
            "JPX component count disagrees with colour space",
        ));
    }
    let pixels = parameters::budget(image.width, image.height, 1, 1, limits)?;
    validate_planes(&decoded, &component_geometry, reference_origin, mode)?;
    let mixed = components.iter().any(|c| c.0.bits != first.0.bits);
    let precision = if mixed { 16 } else { first.0.bits };
    let values = parameters::budget(image.width, image.height, count, precision, limits)?;
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
    image.components = count;
    image.bits_per_component = if mixed { 0 } else { first.0.bits };
    image.samples = interleave(&components, values, mixed, image.width, reference_origin)?;
    if mode != 0 {
        let opacity = decoded
            .alpha
            .and_then(|index| decoded.components.get(index))
            .ok_or(Error::syntax("missing JPX opacity channel"))?;
        image.mask = MaskKind::Embedded(ImageAlpha {
            bits_per_component: opacity.bits,
            samples: interleave(
                &[(opacity, &component_geometry[decoded.alpha.unwrap_or(0)])],
                pixels,
                false,
                image.width,
                reference_origin,
            )?,
            premultiplied: mode == 2,
        });
    }
    if matches!(image.kind, ImageKind::Color { color_space: None }) {
        image.kind = embedded_space(decoded.color, count)?;
    }
    Ok(())
}

fn validate_planes(
    decoded: &jpx::JpxImage,
    geometries: &[jpx::ComponentGeometry],
    reference_origin: [u32; 2],
    mode: i64,
) -> Result<()> {
    for (index, (component, geometry)) in decoded.components.iter().zip(geometries).enumerate() {
        if mode != 0 || Some(index) != decoded.alpha {
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
    validate_placement(c, geometry, (width, height), reference_origin)?;
    if u64::try_from(c.samples.len()).ok() != Some(u64::from(c.width) * u64::from(c.height)) {
        return Err(Error::syntax("invalid JPX sample count"));
    }
    let max = u16::MAX >> (16 - c.bits);
    if c.samples.iter().any(|&v| v > max) {
        return Err(Error::syntax("JPX sample exceeds precision"));
    }
    Ok(())
}

/// The precision and reference-grid placement checks of
/// [`validate_component`], which need no samples.
fn validate_placement(
    c: &jpx::JpxComponent,
    geometry: &jpx::ComponentGeometry,
    (width, height): (u32, u32),
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
    Ok(())
}

fn storage_bytes(
    decoded: &jpx::JpxImage,
    geometry_capacity: usize,
    values: usize,
    bits: u8,
    mode: i64,
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
    if mode != 0
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

fn interleave(
    components: &[(&jpx::JpxComponent, &jpx::ComponentGeometry)],
    count: usize,
    mixed: bool,
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
        let x = u64::from(reference_origin[0]) + (pixel % width as usize) as u64;
        let y = u64::from(reference_origin[1]) + (pixel / width as usize) as u64;
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
    if !mixed && first.bits <= 8 {
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
    if mixed {
        Ok(Samples::Mixed {
            values,
            bits: components.iter().map(|c| c.0.bits).collect(),
        })
    } else {
        Ok(Samples::Words(values))
    }
}

fn embedded_space(color: jpx::JpxColor, count: u8) -> Result<ImageKind> {
    let space = match (&color, count) {
        (jpx::JpxColor::Enumerated(17), 1) => Some(b"DeviceGray".as_slice()),
        (jpx::JpxColor::Enumerated(16), 3) => Some(b"DeviceRGB".as_slice()),
        (jpx::JpxColor::Enumerated(12), 4) => Some(b"DeviceCMYK".as_slice()),
        (jpx::JpxColor::Enumerated(12 | 16 | 17), _) => {
            return Err(Error::syntax(
                "JPX colour space disagrees with channel count",
            ));
        }
        (jpx::JpxColor::Enumerated(19), _) => {
            return Err(Error::syntax("CIEJab is forbidden for PDF JPX images"));
        }
        (jpx::JpxColor::Enumerated(18), 3) | (jpx::JpxColor::Icc(_), _) => None,
        (jpx::JpxColor::Enumerated(18), _) => {
            return Err(Error::syntax("JPX sYCC requires three colour channels"));
        }
        (jpx::JpxColor::Enumerated(_), _) => {
            return Err(Error::unsupported(
                features::JPX_COLOUR,
                "JPX enumerated colour space is not implemented",
            ));
        }
        (jpx::JpxColor::Unspecified, _) => {
            return Err(Error::syntax(
                "JPX without dictionary ColorSpace requires container colour information",
            ));
        }
    };
    Ok(space.map_or_else(
        || ImageKind::EmbeddedColor { color },
        |space| ImageKind::Color {
            color_space: Some(Value::name(space)),
        },
    ))
}
