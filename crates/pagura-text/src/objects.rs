//! Resolved font dictionary access.
use pagura_core::{Dictionary, Error, Result, Value};
use pagura_document::Document;
use pagura_document::source::ReadResult;

pub(crate) fn optional<'a>(
    document: &'a Document,
    dict: &'a Dictionary,
    key: &[u8],
) -> ReadResult<Option<&'a Value>> {
    dict.get(key)
        .map(|v| document.resolve(v))
        .transpose()
        .map(|v| v.filter(|v| !v.is_null()))
}
pub(crate) fn required<'a>(
    document: &'a Document,
    dict: &'a Dictionary,
    key: &[u8],
) -> ReadResult<&'a Value> {
    optional(document, dict, key)?.ok_or_else(|| Error::syntax("missing required font entry"))
}
pub(crate) fn array(value: &Value) -> Result<&[Value]> {
    value
        .as_array()
        .ok_or_else(|| Error::syntax("font entry is not an array"))
}
/// An array of exactly `N` numbers, each resolved (7.3.6).
pub(crate) fn numbers<const N: usize>(
    document: &Document,
    value: &Value,
    message: &'static str,
) -> ReadResult<[f64; N]> {
    let values: &[Value; N] = array(value)?
        .try_into()
        .map_err(|_| Error::syntax(message))?;
    let mut numbers = [0.0; N];
    for (slot, value) in numbers.iter_mut().zip(values) {
        *slot = document
            .resolve(value)?
            .as_number()
            .ok_or(Error::syntax(message))?;
    }
    Ok(numbers)
}

pub(crate) fn number(
    document: &Document,
    dict: &Dictionary,
    key: &[u8],
    default: f64,
) -> ReadResult<f64> {
    optional(document, dict, key)?.map_or(Ok(default), |v| {
        v.as_number()
            .ok_or_else(|| Error::syntax("font entry is not a finite number"))
    })
}
