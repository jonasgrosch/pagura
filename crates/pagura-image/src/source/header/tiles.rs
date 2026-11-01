//! Direct, complete metadata/data admission; other XObjects retain the old path.

use super::{Codec, Header};
use crate::source::tiles::Admission;
use crate::{ImageKind, ImageSource, SourceMask};
use pagura_core::convert::{u32_to_usize, usize_to_u64};
use pagura_core::{Dictionary, Error, Limits, Meter, Result, Value};
use pagura_document::{Document, IndirectObject};
use std::borrow::Cow;

/// No lookup or allocation occurs while deciding eligibility. Indirect or
/// malformed metadata falls back to the original constructor, preserving its
/// validation/refusal order. Unused metadata is never cloned by this route.
fn eligible(dict: &Dictionary) -> bool {
    if dict
        .get(b"Subtype")
        .and_then(Value::as_name)
        .is_none_or(|name| name != b"Image")
        || !matches!(dict.get(b"Type"), None | Some(Value::Null))
            && !dict.is_name(b"Type", b"XObject")
        || dict.contains_key(b"F")
        || dict.contains_key(b"DecodeParms")
        || dict.contains_key(b"Mask")
        || dict.contains_key(b"SMask")
        || dict.contains_key(b"Matte")
        || !matches!(
            dict.get(b"ImageMask"),
            None | Some(Value::Null | Value::Boolean(false))
        )
        || !dict.is_name(b"ColorSpace", b"DeviceGray")
        || dict.get(b"BitsPerComponent").and_then(Value::as_integer) != Some(1)
        || !matches!(
            dict.get(b"SMaskInData"),
            None | Some(Value::Null | Value::Integer(0))
        )
        || !matches!(
            dict.get(b"Interpolate"),
            None | Some(Value::Null | Value::Boolean(_))
        )
    {
        return false;
    }
    let intent = match dict.get(b"Intent") {
        None | Some(Value::Null) => true,
        Some(Value::Name(name)) => matches!(
            name.as_bytes(),
            b"AbsoluteColorimetric" | b"RelativeColorimetric" | b"Perceptual" | b"Saturation"
        ),
        _ => false,
    };
    let decode = match dict.get(b"Decode") {
        None | Some(Value::Null) => true,
        Some(Value::Array(values)) if values.len() == 2 => values
            .iter()
            .all(|value| value.as_number().is_some_and(f64::is_finite)),
        _ => false,
    };
    let filter = match dict.get(b"Filter") {
        None | Some(Value::Null) => true,
        Some(Value::Name(name)) => name == b"FlateDecode",
        Some(Value::Array(values)) => {
            values.is_empty()
                || matches!(values.as_slice(), [Value::Name(name)] if name == b"FlateDecode")
        }
        _ => false,
    };
    intent && decode && filter
}

fn dimensions(dict: &Dictionary) -> Option<(u32, u32)> {
    let dimension = |key| {
        dict.get(key)
            .and_then(Value::as_integer)
            .and_then(|value| u32::try_from(value).ok())
            .filter(|&value| value > 0)
    };
    Some((dimension(b"Width")?, dimension(b"Height")?))
}

fn budget(width: u32, height: u32, limits: &Limits) -> Result<usize> {
    let width = u32_to_usize(width);
    let height = u32_to_usize(height);
    Meter::new(
        u32_to_usize(limits.tiled_images.samples),
        "tiled image sample budget exceeded",
    )
    .charge_product(width, height)?;
    if width.min(height) > limits.pixels {
        return Err(Error::limit("tiled image row exceeds surface pixel budget"));
    }
    let bytes = width
        .div_ceil(8)
        .checked_mul(height)
        .ok_or(Error::limit("tiled image source byte count overflow"))?;
    Meter::new(
        limits.decoded_bytes,
        "tiled image source byte budget exceeded",
    )
    .charge(bytes)?;
    Ok(bytes)
}

pub(super) fn read<'a>(
    document: &'a Document,
    image: &IndirectObject,
) -> Result<Option<(Header, Cow<'a, [u8]>)>> {
    let Some(dict) = image.dictionary() else {
        return Ok(None);
    };
    let Some((width, height)) = dimensions(dict) else {
        return Ok(None);
    };
    let limits = document.limits();
    if u64::from(width) * u64::from(height) <= usize_to_u64(limits.pixels) || !eligible(dict) {
        return Ok(None);
    }
    let expected = budget(width, height, &limits)?;
    let encoded_bytes = document.raw_stream_data(image)?.len();
    // One complete excess byte can be identified as a row mismatch. Longer
    // excess data stays capped rather than inflating to the document maximum.
    let bound = expected
        .checked_add(1)
        .ok_or(Error::limit("tiled image source bound overflow"))?
        .min(limits.decoded_bytes);
    let mut auxiliary = Meter::new(
        limits.decode_bytes,
        "tiled image source auxiliary budget exceeded",
    );
    auxiliary.charge_product(encoded_bytes, 2)?;
    // Bounded Flate output/growth and its Cow-to-Arc transition. Generic
    // filter entropy work retains its stream domain. Fixed Rust call-stack
    // and allocator bookkeeping are outside the owned-capacity model.
    auxiliary.charge_product(bound, 3)?;
    auxiliary.charge_items::<ImageSource>(1)?;
    auxiliary.charge_items::<Header>(1)?;
    auxiliary.charge_items::<Admission>(1)?;
    // A one-byte precision vector, two-value Decode vector and shared-owner
    // metadata, reserved before their bounded constructors run.
    auxiliary.charge_items::<u8>(8)?;
    auxiliary.charge_items::<f64>(4)?;
    auxiliary.charge_items::<usize>(4)?;
    let stream = document.image_stream_with_strict_filters(image, bound)?;
    if stream.codec.is_some() || stream.data.len() != expected {
        return Err(Error::syntax(
            "tiled mask requires exact complete sample rows",
        ));
    }
    let kind = ImageKind::Color {
        color_space: Some(Value::name(b"DeviceGray")),
    };
    let mut header = Header {
        width,
        height,
        declared_components: 1,
        declared_bits: 1,
        components: 1,
        bits: vec![1],
        initial: kind.clone(),
        kind,
        decode: None,
        interpolate: false,
        mask: SourceMask::None,
        intent: None,
        codec: Codec::None,
        stencil_after_decode: false,
    };
    super::finish(document, dict, &mut header)?;
    header.codec = Codec::TiledMask(Box::new(Admission::new(&limits, encoded_bytes)));
    Ok(Some((header, stream.data)))
}
