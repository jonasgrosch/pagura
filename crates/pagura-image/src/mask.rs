//! Mask identity, precedence and colour-key validation (8.9.6).
use crate::MaskKind;
use pagura_core::{Dictionary, Error, Result, Value};
use pagura_document::Document;

pub(crate) fn read(
    document: &Document,
    dict: &Dictionary,
    image: &crate::DecodedImage,
) -> Result<MaskKind> {
    read_for(document, dict, image.components, &|component| {
        image.component_bits(component)
    })
}

/// [`read`] for an image of `components` colour components whose
/// precisions `bits` gives, before its samples are decoded.
pub(crate) fn read_for(
    document: &Document,
    dict: &Dictionary,
    components: u8,
    bits: &dyn Fn(usize) -> Option<u8>,
) -> Result<MaskKind> {
    if let Some(value) = dict.get(b"SMask").filter(|v| !v.is_null()) {
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
            || mask.contains_key(b"Mask")
            || mask.contains_key(b"SMask")
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
        return Ok(MaskKind::Soft(id));
    } else if let Some(value) = dict.get(b"Mask").filter(|v| !v.is_null()) {
        let stream = value
            .as_reference()
            .map(|id| document.object(id))
            .transpose()?
            .filter(|o| o.is_stream());
        if let Some(object) = stream {
            let id = object.id();
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
            return Ok(MaskKind::Stencil(id));
        }
        let ranges = document
            .resolve(value)?
            .as_array()
            .ok_or(Error::syntax("invalid image Mask"))?;
        if ranges.len() != usize::from(components) * 2 {
            return Err(Error::syntax("invalid colour key length"));
        }
        let mut keys = Vec::with_capacity(usize::from(components));
        for (component, pair) in ranges.as_chunks::<2>().0.iter().enumerate() {
            let bits = bits(component).ok_or(Error::syntax("missing image component precision"))?;
            let max = u16::MAX >> (16 - bits);
            let sample = |v| {
                u16::try_from(
                    document
                        .resolve(v)?
                        .as_integer()
                        .ok_or(Error::syntax("invalid colour key"))?,
                )
                .ok()
                .filter(|&v| v <= max)
                .ok_or(Error::syntax("colour key out of range"))
            };
            let (low, high) = (sample(&pair[0])?, sample(&pair[1])?);
            if low > high {
                return Err(Error::syntax("reversed colour key range"));
            }
            keys.push((low, high));
        }
        return Ok(MaskKind::ColorKey(keys));
    }
    Ok(MaskKind::None)
}
