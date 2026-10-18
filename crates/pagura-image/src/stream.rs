//! Narrow parameter recovery before image dispatch (7.3.8.1 Table 5, 7.4.1).
use pagura_core::{Diagnostic, Diagnostics, Error, Position, Result, Value};
use pagura_document::{Document, ImageStream, IndirectObject};

pub(crate) fn read<'a>(
    document: &'a Document,
    image: &IndirectObject,
    diagnostics: &mut Diagnostics,
) -> Result<ImageStream<'a>> {
    let dictionary = image
        .dictionary()
        .ok_or(Error::syntax("image has no dictionary"))?;
    let Some(params) = dictionary.get(b"DecodeParms") else {
        return document.image_stream(image);
    };
    let params = document.resolve(params)?;
    let filter = dictionary
        .get(b"Filter")
        .map(|v| document.resolve(v))
        .transpose()?;
    let names = match filter {
        Some(value @ Value::Name(_)) => std::slice::from_ref(value),
        Some(Value::Array(names)) => names,
        _ => return document.image_stream(image),
    };
    let Some(params) = repaired_params(document, names, params)? else {
        return document.image_stream(image);
    };
    diagnostics.repair(
        Diagnostic::new(
            crate::tags::DECODE_PARAMS,
            "malformed image DecodeParms entry treated as absent (7.3.8.1 Table 5, 7.4.1, 7.4.9)",
        )
        .at(Position::Stream {
            object: image.id(),
            offset: 0,
        }),
    )?;
    document.image_stream_with_parameters(image, Some(&params))
}

/// Preserve array alignment and all dictionaries. A string spelling `null`
/// has an unambiguous intended default; arbitrary wrong types are discarded
/// only for JPXDecode, whose interpretation has no external parameters (7.4.9).
fn repaired_params(document: &Document, names: &[Value], params: &Value) -> Result<Option<Value>> {
    let entries = match params {
        Value::Array(entries) if entries.len() == names.len() => entries.as_slice(),
        Value::Null | Value::Array(_) => return Ok(None),
        value if names.len() == 1 => std::slice::from_ref(value),
        _ => return Ok(None),
    };
    let mut repaired = false;
    let mut normalized = Vec::with_capacity(entries.len());
    for (name, value) in names.iter().zip(entries) {
        let name = document.resolve(name)?;
        let value = document.resolve(value)?;
        let invalid = !matches!(value, Value::Null | Value::Dictionary(_));
        let absent =
            value.as_string() == Some(b"null") || name.as_name().is_some_and(|n| n == b"JPXDecode");
        if invalid && absent {
            repaired = true;
            normalized.push(Value::Null);
        } else {
            normalized.push(value.clone());
        }
    }
    Ok(repaired.then(|| match params {
        Value::Array(_) => Value::Array(normalized),
        _ => normalized.pop().unwrap_or(Value::Null),
    }))
}
