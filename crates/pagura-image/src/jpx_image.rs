//! PDF JPEG 2000 adaptation: ISO 32000-2 7.4.9 and Table 87.
use crate::{DecodedImage, ImageAlpha, ImageKind, MaskKind, Samples, features, jpx, parameters};
use pagura_core::{
    Diagnostic, Diagnostics, Dictionary, Error, Limits, Meter, Position, Recovery, Result, Value,
};

/// How a PDF JPXDecode image is decoded: its `SMaskInData` mode and the
/// container metadata it ignores (7.4.9, Table 87).
#[derive(Debug, Clone)]
pub(crate) struct Jpx {
    pub(crate) mode: i64,
    options: jpx::Options,
    indexed: bool,
    raw: bool,
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
    pub(crate) fn read(
        data: &[u8],
        dictionary: &Dictionary,
        kind: &ImageKind,
        recovery: Recovery,
    ) -> Result<Self> {
        let mode = parameters::integer(dictionary, b"SMaskInData", 0)?;
        if !(0..=2).contains(&mode)
            || (mode != 0 && dictionary.get(b"SMask").is_some_and(|v| !v.is_null()))
        {
            return Err(Error::syntax("invalid SMaskInData or simultaneous SMask"));
        }
        let raw = data.starts_with(&[255, 79]);
        if raw {
            // A raw codestream supplies dimensions and precision, but no
            // colour/channel associations. Require the PDF to supply colour
            // interpretation and refuse opacity that needs container metadata.
            if mode != 0
                || !matches!(
                    kind,
                    ImageKind::Color {
                        color_space: Some(_)
                    }
                )
            {
                return Err(Error::syntax(
                    "raw PDF JPX data requires an explicit colour space and no embedded opacity",
                ));
            }
            // Description checks policy without reporting it. Decoding below
            // forwards the repair to the caller's bounded diagnostic sink.
            if recovery == Recovery::Strict {
                report_raw(&mut Diagnostics::new(recovery))?;
            }
        }
        let options = jpx::Options {
            ignore_color: !matches!(kind, ImageKind::Color { color_space: None }),
            ignore_opacity: mode == 0,
        };
        let indexed = matches!(kind, ImageKind::Color { color_space: Some(Value::Array(space)) }
            if space.first().and_then(Value::as_name).is_some_and(|name| name == b"Indexed"));
        Ok(Self {
            mode,
            options,
            indexed,
            raw,
        })
    }

    pub(crate) fn decode(
        &self,
        data: &[u8],
        image: &mut DecodedImage,
        limits: &Limits,
        diagnostics: &mut pagura_core::Diagnostics,
    ) -> Result<()> {
        if self.raw {
            report_raw(diagnostics)?;
        }
        let mut decoded = jpx::decode_pdf(data, limits, &self.options, diagnostics, self.indexed)?;
        if self.mode == 0 && !self.raw {
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
        if self.mode == 0 && !self.raw {
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
            .then(|| embedded_space(decoded.color, count, colours.iter().any(|c| c.0.signed)))
            .transpose()?;
        Ok(Description {
            bits: colours.iter().map(|c| c.0.bits).collect(),
            kind,
        })
    }
}

fn report_raw(diagnostics: &mut Diagnostics) -> Result<()> {
    diagnostics.repair_once(
        Diagnostic::new(
            crate::tags::JPX_CODESTREAM,
            "unboxed JPEG 2000 data uses the explicit PDF colour space (7.4.9)",
        )
        .at(Position::Buffer(0)),
    )
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
    let scratch_bytes = limits.decoded_bytes.saturating_sub(storage);
    image.samples = interleave(
        &components,
        values,
        mixed,
        image.width,
        reference_origin,
        scratch_bytes,
    )?;
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
                scratch_bytes,
            )?,
            premultiplied: mode == 2,
        });
    }
    if matches!(image.kind, ImageKind::Color { color_space: None }) {
        image.kind = embedded_space(decoded.color, count, components.iter().any(|c| c.0.signed))?;
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
    scratch_bytes: usize,
) -> Result<Samples> {
    let first = components
        .first()
        .ok_or(Error::syntax("missing JPX components"))?
        .0;
    let Some(mut scratch) =
        InterleaveScratch::new(components, width, reference_origin, scratch_bytes)
    else {
        return interleave_plain(components, count, mixed, width, reference_origin);
    };
    if !mixed && first.bits <= 8 {
        let mut values = Vec::with_capacity(count);
        each_sample(
            components,
            count,
            width,
            reference_origin,
            &mut scratch,
            |sample| {
                values.push(
                    u8::try_from(sample)
                        .map_err(|_| Error::syntax("JPX sample exceeds byte precision"))?,
                );
                Ok(())
            },
        )?;
        return Ok(Samples::Bytes(values));
    }
    let mut values = Vec::with_capacity(count);
    each_sample(
        components,
        count,
        width,
        reference_origin,
        &mut scratch,
        |sample| {
            values.push(sample);
            Ok(())
        },
    )?;
    if mixed {
        Ok(Samples::Mixed {
            values,
            bits: components.iter().map(|c| c.0.bits).collect(),
        })
    } else {
        Ok(Samples::Words(values))
    }
}

/// Optional interleaving scratch. All allocations fit beside every mandatory
/// decoded/output buffer, or the previous sampler is used without scratch.
struct InterleaveScratch<'a> {
    columns: Vec<Vec<usize>>,
    rows: Vec<&'a [u16]>,
}

impl<'a> InterleaveScratch<'a> {
    fn new(
        components: &[(&'a jpx::JpxComponent, &jpx::ComponentGeometry)],
        width: u32,
        reference_origin: [u32; 2],
        available: usize,
    ) -> Option<Self> {
        let width = usize::try_from(width).ok()?;
        let mut bytes = Meter::new(available, "JPX interleaving scratch exceeds budget");
        let mut columns = Vec::new();
        bytes.reserve(&mut columns, components.len()).ok()?;
        let mut rows = Vec::new();
        bytes.reserve(&mut rows, components.len()).ok()?;
        for &(component, geometry) in components {
            let mut column = Vec::new();
            bytes.reserve(&mut column, width).ok()?;
            for x in 0..width {
                let at = u64::from(reference_origin[0]) + u64::try_from(x).ok()?;
                let sample = (at.saturating_sub(u64::from(geometry.origin[0]))
                    / u64::from(geometry.step[0]))
                .min(u64::from(component.width.saturating_sub(1)));
                column.push(usize::try_from(sample).ok()?);
            }
            columns.push(column);
        }
        Some(Self { columns, rows })
    }
}

/// Passes `take` the `count` samples of `components` in image order: pixel
/// by pixel, each pixel's components in turn. T.800 B.2/G.4 fix the sample
/// positions. Piecewise-constant expansion with border replication is the
/// chosen resampling policy; no phase reset occurs at a nonzero
/// reference-grid origin. Each component's sample column is found once for
/// every image column, and its sample row once a row.
fn each_sample<'a>(
    components: &[(&'a jpx::JpxComponent, &jpx::ComponentGeometry)],
    count: usize,
    width: u32,
    reference_origin: [u32; 2],
    scratch: &mut InterleaveScratch<'a>,
    mut take: impl FnMut(u16) -> Result<()>,
) -> Result<()> {
    let overflow = || Error::limit("JPX sample index overflow");
    let place = |at: u64, origin: u32, step: u32, size: u32| {
        (at.saturating_sub(u64::from(origin)) / u64::from(step))
            .min(u64::from(size.saturating_sub(1)))
    };
    let pixels = count / components.len().max(1);
    let image_width = usize::try_from(width).map_err(|_| overflow())?;
    if image_width == 0 {
        return Ok(());
    }
    let InterleaveScratch { columns, rows } = scratch;
    for (row, start) in (0u64..).zip((0..pixels).step_by(image_width)) {
        rows.clear();
        for &(c, g) in components {
            let y = place(
                u64::from(reference_origin[1]) + row,
                g.origin[1],
                g.step[1],
                c.height,
            );
            let span = usize::try_from(c.width).map_err(|_| overflow())?;
            let first = usize::try_from(y * u64::from(c.width)).map_err(|_| overflow())?;
            rows.push(
                c.samples
                    .get(first..first + span)
                    .ok_or(Error::syntax("invalid JPX sample index"))?,
            );
        }
        for x in 0..(pixels - start).min(image_width) {
            for (samples, columns) in rows.iter().zip(columns.iter()) {
                take(samples[columns[x]])?;
            }
        }
    }
    Ok(())
}

fn interleave_plain(
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

fn embedded_space(color: jpx::JpxColor, count: u8, signed: bool) -> Result<ImageKind> {
    let space = match (&color, count) {
        (jpx::JpxColor::Enumerated(18), _) if signed => {
            return Err(Error::syntax("JPX sYCC requires unsigned colour channels"));
        }
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

#[cfg(test)]
mod row_scratch_tests {
    use super::*;

    fn placed(mixed: bool) -> jpx::Decoded {
        let origin = [1_u32, 2_u32];
        let mut components = Vec::new();
        let mut geometry = Vec::new();
        for channel in 0..4_u32 {
            let step = if channel == 3 { 1 } else { channel + 1 };
            let width = (origin[0] + 11).div_ceil(step) - origin[0].div_ceil(step);
            let height = (origin[1] + 7).div_ceil(step) - origin[1].div_ceil(step);
            let bits = if mixed && channel == 1 { 12 } else { 8 };
            let count = usize::try_from(width * height).unwrap();
            let range = usize::from(u16::MAX >> (16 - bits)) + 1;
            let samples = (0..count)
                .map(|i| {
                    u16::try_from((i * 13 + usize::try_from(channel).unwrap() * 37) % range)
                        .unwrap()
                })
                .collect();
            components.push(jpx::JpxComponent {
                bits,
                signed: false,
                width,
                height,
                samples,
            });
            geometry.push(jpx::ComponentGeometry {
                step: [step, step],
                origin: [
                    origin[0].div_ceil(step) * step,
                    origin[1].div_ceil(step) * step,
                ],
            });
        }
        jpx::Decoded {
            image: jpx::JpxImage {
                width: 11,
                height: 7,
                components,
                color: jpx::JpxColor::Enumerated(16),
                alpha: Some(3),
            },
            reference_origin: origin,
            component_geometry: geometry,
            opacity: None,
        }
    }

    fn scratch_bytes(width: usize, components: usize) -> usize {
        components * (size_of::<Vec<usize>>() + size_of::<&[u16]>() + width * size_of::<usize>())
    }

    #[test]
    fn optional_interleave_scratch_is_charged_before_allocation() {
        let decoded = placed(false);
        let components: Vec<_> = decoded
            .image
            .components
            .iter()
            .zip(&decoded.component_geometry)
            .take(3)
            .collect();
        let bytes = scratch_bytes(11, components.len());
        assert!(InterleaveScratch::new(&components, 11, decoded.reference_origin, 0).is_none());
        assert!(
            InterleaveScratch::new(&components, 11, decoded.reference_origin, bytes - 1).is_none()
        );
        let scratch =
            InterleaveScratch::new(&components, 11, decoded.reference_origin, bytes).unwrap();
        let held = scratch.columns.capacity() * size_of::<Vec<usize>>()
            + scratch.rows.capacity() * size_of::<&[u16]>()
            + scratch
                .columns
                .iter()
                .map(|c| c.capacity() * size_of::<usize>())
                .sum::<usize>();
        assert!(held <= bytes);
    }

    #[test]
    fn optional_interleave_scratch_keeps_subsampled_mixed_and_byte_values() {
        for mixed in [false, true] {
            let decoded = placed(mixed);
            let components: Vec<_> = decoded
                .image
                .components
                .iter()
                .zip(&decoded.component_geometry)
                .take(3)
                .collect();
            let expected =
                interleave_plain(&components, 11 * 7 * 3, mixed, 11, decoded.reference_origin)
                    .unwrap();
            for bytes in [0, scratch_bytes(11, components.len())] {
                assert_eq!(
                    interleave(
                        &components,
                        11 * 7 * 3,
                        mixed,
                        11,
                        decoded.reference_origin,
                        bytes
                    )
                    .unwrap(),
                    expected
                );
            }
        }
    }

    #[test]
    fn full_jpx_adapter_keeps_its_previous_minimum_storage_budget() {
        for mode in [0, 1, 2] {
            let decoded = placed(false);
            let components: Vec<_> = decoded
                .image
                .components
                .iter()
                .zip(&decoded.component_geometry)
                .take(3)
                .collect();
            let expected =
                interleave_plain(&components, 11 * 7 * 3, false, 11, decoded.reference_origin)
                    .unwrap();
            let bytes = storage_bytes(
                &decoded.image,
                decoded.component_geometry.capacity(),
                11 * 7 * 3,
                8,
                mode,
            )
            .unwrap();
            for extra in [0, scratch_bytes(11, components.len())] {
                let mut image = DecodedImage {
                    width: 11,
                    height: 7,
                    bits_per_component: 8,
                    components: 3,
                    samples: Samples::Bytes(Vec::new()),
                    kind: ImageKind::EmbeddedColor {
                        color: jpx::JpxColor::Enumerated(16),
                    },
                    decode: None,
                    mask: MaskKind::None,
                    interpolate: false,
                };
                let limits = Limits {
                    decoded_bytes: bytes + extra,
                    ..Limits::default()
                };
                adapt(decoded.clone(), mode, &mut image, &limits).unwrap();
                assert_eq!(image.samples, expected);
                if mode != 0 {
                    let alpha = interleave_plain(
                        &[(&decoded.image.components[3], &decoded.component_geometry[3])],
                        11 * 7,
                        false,
                        11,
                        decoded.reference_origin,
                    )
                    .unwrap();
                    assert_eq!(
                        image.mask,
                        MaskKind::Embedded(ImageAlpha {
                            samples: alpha,
                            bits_per_component: 8,
                            premultiplied: mode == 2
                        })
                    );
                }
            }
        }
    }
}
