//! PDF JPEG 2000 adaptation: ISO 32000-2 7.4.9 and Table 87.
use crate::{DecodedImage, ImageAlpha, ImageKind, MaskKind, Samples, features, jpx, parameters};
use pagura_core::{Dictionary, Error, Limits, Result, Value};

pub(crate) fn decode(
    data: &[u8],
    dictionary: &Dictionary,
    image: &mut DecodedImage,
    limits: &Limits,
) -> Result<()> {
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
    let decoded = jpx::decode(data, limits)?;
    adapt(decoded, mode, image, limits)
}

pub(crate) fn adapt(
    decoded: jpx::JpxImage,
    mode: i64,
    image: &mut DecodedImage,
    limits: &Limits,
) -> Result<()> {
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
    if storage_bytes(&decoded, 0, 8, 0)? > limits.decoded_bytes {
        return Err(Error::limit("JPX component adapter budget exceeded"));
    }
    let components: Vec<_> = decoded
        .components
        .iter()
        .enumerate()
        .filter(|(index, _)| Some(*index) != decoded.alpha)
        .map(|(_, c)| c)
        .collect();
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
    for component in &components {
        validate_component(component, image.width, image.height, pixels)?;
    }
    if mode != 0
        && let Some(alpha) = decoded
            .alpha
            .and_then(|index| decoded.components.get(index))
    {
        validate_component(alpha, image.width, image.height, pixels)?;
    }
    let mixed = components.iter().any(|c| c.bits != first.bits);
    let precision = if mixed { 16 } else { first.bits };
    let values = parameters::budget(image.width, image.height, count, precision, limits)?;
    let storage = storage_bytes(&decoded, values, precision, mode)?;
    if storage > limits.decoded_bytes {
        return Err(Error::limit("JPX adapter sample budget exceeded"));
    }
    image.components = count;
    image.bits_per_component = if mixed { 0 } else { first.bits };
    image.samples = interleave(&components, values, mixed)?;
    if mode != 0 {
        let opacity = decoded
            .alpha
            .and_then(|index| decoded.components.get(index))
            .ok_or(Error::syntax("missing JPX opacity channel"))?;
        image.mask = MaskKind::Embedded(ImageAlpha {
            bits_per_component: opacity.bits,
            samples: interleave(&[opacity], pixels, false)?,
            premultiplied: mode == 2,
        });
    }
    if matches!(image.kind, ImageKind::Color { color_space: None }) {
        image.kind = embedded_space(decoded.color, count)?;
    }
    Ok(())
}

fn validate_component(c: &jpx::JpxComponent, width: u32, height: u32, pixels: usize) -> Result<()> {
    if !(1..=16).contains(&c.bits) {
        return Err(Error::unsupported(
            features::JPX_SAMPLE_LAYOUT,
            "JPX precision exceeds sample representation",
        ));
    }
    if c.width != width || c.height != height {
        return Err(Error::unsupported(
            features::JPX_SAMPLE_LAYOUT,
            "JPX component subsampling requires reference-grid geometry",
        ));
    }
    if c.samples.len() != pixels {
        return Err(Error::syntax("invalid JPX sample count"));
    }
    let max = u16::MAX >> (16 - c.bits);
    if c.samples.iter().any(|&v| v > max) {
        return Err(Error::syntax("JPX sample exceeds precision"));
    }
    Ok(())
}

fn storage_bytes(decoded: &jpx::JpxImage, values: usize, bits: u8, mode: i64) -> Result<usize> {
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
            alpha
                .samples
                .len()
                .checked_mul(if alpha.bits <= 8 { 1 } else { 2 })
                .ok_or(Error::limit("JPX alpha storage overflow"))?,
        )?;
    }
    if let jpx::JpxColor::Icc(profile) = &decoded.color {
        bytes = add(bytes, profile.len())?;
    }
    let metadata = decoded
        .components
        .len()
        .checked_mul(size_of::<jpx::JpxComponent>() + size_of::<&jpx::JpxComponent>() + 1)
        .ok_or(Error::limit("JPX component storage overflow"))?;
    add(bytes, metadata)
}

fn interleave(components: &[&jpx::JpxComponent], count: usize, mixed: bool) -> Result<Samples> {
    let first = components
        .first()
        .ok_or(Error::syntax("missing JPX components"))?;
    let pixels = first.samples.len();
    if !mixed && first.bits <= 8 {
        let mut values = Vec::with_capacity(count);
        for i in 0..pixels {
            for c in components {
                values.push(
                    u8::try_from(c.samples[i])
                        .map_err(|_| Error::syntax("JPX sample exceeds byte precision"))?,
                );
            }
        }
        return Ok(Samples::Bytes(values));
    }
    let mut values = Vec::with_capacity(count);
    for i in 0..pixels {
        for c in components {
            values.push(c.samples[i]);
        }
    }
    if mixed {
        Ok(Samples::Mixed {
            values,
            bits: components.iter().map(|c| c.bits).collect(),
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
