//! Narrow parameter recovery before image dispatch (7.3.8.1 Table 5, 7.4.1).
use pagura_codec::FilterChain;
use pagura_core::{Diagnostic, Diagnostics, Dictionary, Error, Position, Result, Value};
use pagura_document::{Document, ImageStream, IndirectObject};
use std::borrow::Cow;

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
        .map(|v| resolve_entries(document, v))
        .transpose()?;
    let names = match filter.as_ref() {
        Some(value @ Value::Name(_)) => std::slice::from_ref(value),
        Some(Value::Array(names)) => names,
        _ => return document.image_stream(image),
    };
    let Some(params) = repaired_params(document, names, params)? else {
        return document.image_stream(image);
    };
    // The document API owns decryption and Crypt selection. It does not yet
    // accept normalized parameters. Never interpret ciphertext as codec data,
    // bypass external streams, or duplicate the security handler here.
    if dictionary.contains_key(b"F")
        || document
            .trailer()
            .get(b"Encrypt")
            .map(|v| document.resolve(v))
            .transpose()?
            .is_some_and(|v| !v.is_null())
        || names
            .iter()
            .any(|v| v.as_name().is_some_and(|n| n == b"Crypt"))
    {
        return document.image_stream(image);
    }
    let mut chain = FilterChain::from_entries(filter.as_ref(), Some(&params))?;
    diagnostics.repair(
        Diagnostic::new(
            crate::tags::DECODE_PARAMS,
            "malformed image DecodeParms entry treated as absent (7.3.8.1 Table 5, 7.4.1, 7.4.9)",
        )
        .at(Position::Buffer(0)),
    )?;
    let codec = chain.split_image();
    let raw = document.raw_stream_data(image)?;
    let data = match chain.decode(raw, document.limits().decoded_bytes, diagnostics)? {
        Cow::Borrowed(_) => Cow::Borrowed(raw),
        Cow::Owned(data) => Cow::Owned(data),
    };
    Ok(ImageStream { data, codec })
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
        let value = document.resolve(value)?;
        let invalid = !matches!(value, Value::Null | Value::Dictionary(_));
        let absent =
            value.as_string() == Some(b"null") || name.as_name().is_some_and(|n| n == b"JPXDecode");
        if invalid && absent {
            repaired = true;
            normalized.push(Value::Null);
        } else {
            normalized.push(resolve_entries(document, value)?);
        }
    }
    Ok(repaired.then(|| match params {
        Value::Array(_) => Value::Array(normalized),
        _ => normalized.pop().unwrap_or(Value::Null),
    }))
}

/// Match the document stream API's one-level parameter resolution, retaining
/// limits and reference errors without recursively walking arbitrary values.
fn resolve_entries(document: &Document, value: &Value) -> Result<Value> {
    let entry = |value| -> Result<Value> {
        Ok(match document.resolve(value)? {
            Value::Dictionary(dictionary) => Value::Dictionary(
                dictionary
                    .iter()
                    .map(|(key, value)| Ok((key.clone(), document.resolve(value)?.clone())))
                    .collect::<Result<Dictionary>>()?,
            ),
            value => value.clone(),
        })
    };
    match document.resolve(value)? {
        Value::Array(values) => values
            .iter()
            .map(entry)
            .collect::<Result<Vec<_>>>()
            .map(Value::Array),
        value => entry(value),
    }
}
