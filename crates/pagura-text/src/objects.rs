//! Resolved font dictionary access.
use pagura_core::{Dictionary, Error, Result, Value};
use pagura_document::Document;

pub(crate) fn optional<'a>(
    document: &'a Document,
    dict: &'a Dictionary,
    key: &[u8],
) -> Result<Option<&'a Value>> {
    dict.get(key)
        .map(|v| document.resolve(v))
        .transpose()
        .map(|v| v.filter(|v| !v.is_null()))
}
pub(crate) fn required<'a>(
    document: &'a Document,
    dict: &'a Dictionary,
    key: &[u8],
) -> Result<&'a Value> {
    optional(document, dict, key)?.ok_or_else(|| Error::syntax("missing required font entry"))
}
pub(crate) fn array(value: &Value) -> Result<&[Value]> {
    value
        .as_array()
        .ok_or_else(|| Error::syntax("font entry is not an array"))
}
pub(crate) fn number(
    document: &Document,
    dict: &Dictionary,
    key: &[u8],
    default: f64,
) -> Result<f64> {
    optional(document, dict, key)?.map_or(Ok(default), |v| {
        v.as_number()
            .ok_or_else(|| Error::syntax("font entry is not a finite number"))
    })
}
