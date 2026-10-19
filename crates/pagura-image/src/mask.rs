//! Mask identity, precedence and color-key validation (8.9.6).
use crate::{MaskKind, Precision};
use pagura_core::{Dictionary, Error, Result, Value};
use pagura_document::{Document, IndirectObject};

/// The mask of a color image with `components` components of `precision`:
/// `/SMask` takes precedence over `/Mask` (8.9.6).
pub(crate) fn read(
    document: &Document,
    dict: &Dictionary,
    components: u8,
    precision: &Precision,
) -> Result<MaskKind> {
    if let Some(value) = dict.get(b"SMask").filter(|v| !v.is_null()) {
        return soft(document, value);
    }
    let Some(value) = dict.get(b"Mask").filter(|v| !v.is_null()) else {
        return Ok(MaskKind::None);
    };
    let stream = value
        .as_reference()
        .map(|id| document.object(id))
        .transpose()?
        .filter(|o| o.is_stream());
    match stream {
        Some(object) => stencil(document, object),
        None => color_key(document, value, components, precision),
    }
}

/// A soft-mask image (11.6.5.3).
fn soft(document: &Document, value: &Value) -> Result<MaskKind> {
    let id = value
        .as_reference()
        .ok_or(Error::syntax("SMask must reference an image stream"))?;
    let object = document.object(id)?;
    if !object.is_stream() {
        return Err(Error::syntax("SMask is not a stream"));
    }
    let mask = object
        .dictionary()
        .ok_or(Error::syntax("soft mask has no dictionary"))?;
    if mask
        .get(b"ImageMask")
        .map(|v| document.resolve(v))
        .transpose()?
        .and_then(Value::as_bool)
        == Some(true)
        || [b"Mask".as_slice(), b"SMask"]
            .into_iter()
            .any(|key| mask.get(key).is_some_and(|v| !v.is_null()))
    {
        return Err(Error::syntax("invalid soft mask image dictionary"));
    }
    let space = mask
        .get(b"ColorSpace")
        .map(|v| document.resolve(v))
        .transpose()?;
    if space
        .and_then(Value::as_name)
        .is_none_or(|n| n != b"DeviceGray")
    {
        return Err(Error::syntax("soft mask must use DeviceGray"));
    }
    Ok(MaskKind::Soft(id))
}

/// An explicit stencil mask (8.9.6.3).
fn stencil(document: &Document, object: &IndirectObject) -> Result<MaskKind> {
    let mask = object
        .dictionary()
        .ok_or(Error::syntax("mask has no dictionary"))?;
    if !mask
        .get(b"ImageMask")
        .map(|v| document.resolve(v))
        .transpose()?
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err(Error::syntax("explicit mask is not a stencil image"));
    }
    Ok(MaskKind::Stencil(object.id()))
}

/// Color-key masking (8.9.6.4): an inclusive range per component.
fn color_key(
    document: &Document,
    value: &Value,
    components: u8,
    precision: &Precision,
) -> Result<MaskKind> {
    let ranges = document
        .resolve(value)?
        .as_array()
        .ok_or(Error::syntax("invalid image Mask"))?;
    if ranges.len() != usize::from(components) * 2 {
        return Err(Error::syntax("invalid color key length"));
    }
    let mut keys = Vec::with_capacity(usize::from(components));
    for (component, pair) in ranges.as_chunks::<2>().0.iter().enumerate() {
        let bits = match precision {
            Precision::Uniform(bits) => Some(*bits),
            Precision::Mixed(bits) => bits.get(component).copied(),
        }
        .ok_or(Error::syntax("missing image component precision"))?;
        let max = u16::MAX >> (16 - bits);
        let sample = |v| {
            u16::try_from(
                document
                    .resolve(v)?
                    .as_integer()
                    .ok_or(Error::syntax("invalid color key"))?,
            )
            .ok()
            .filter(|&v| v <= max)
            .ok_or(Error::syntax("color key out of range"))
        };
        let (low, high) = (sample(&pair[0])?, sample(&pair[1])?);
        if low > high {
            return Err(Error::syntax("reversed color key range"));
        }
        keys.push(low..=high);
    }
    Ok(MaskKind::ColorKey(keys))
}
