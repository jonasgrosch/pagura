//! Bounded reads of dictionary entries and numbers.

use pagura_core::{Dictionary, Error, Value};
use pagura_document::Document;
use pagura_document::source::ReadResult;

/// The resolved entry `key`; null values, including indirect null, are
/// absent (7.3.9).
pub(crate) fn optional<'a>(
    document: &'a Document,
    dictionary: &'a Dictionary,
    key: &[u8],
) -> ReadResult<Option<&'a Value>> {
    dictionary
        .get(key)
        .map(|value| document.resolve(value))
        .transpose()
        .map(|value| value.filter(|value| !value.is_null()))
}

/// [`optional`] for an entry that must be a dictionary when present.
pub(crate) fn optional_dictionary<'a>(
    document: &'a Document,
    dictionary: &'a Dictionary,
    key: &[u8],
) -> ReadResult<Option<&'a Dictionary>> {
    optional(document, dictionary, key)?
        .map(|value| {
            value
                .as_dictionary()
                .ok_or(Error::syntax("form appearance entry must be a dictionary"))
        })
        .transpose()
}

/// Resolves one finite number.
pub(crate) fn number(document: &Document, value: &Value) -> ReadResult<f64> {
    let number = document
        .resolve(value)?
        .as_number()
        .ok_or(Error::syntax("array entry must be a number"))?;
    if !number.is_finite() {
        return Err(Error::limit("number exceeds numeric working range").into());
    }
    Ok(number)
}

/// Resolves an array of finite numbers of any length. The array's length
/// was bounded when the document parsed it.
pub(crate) fn numbers(document: &Document, value: &Value) -> ReadResult<Vec<f64>> {
    let values = document
        .resolve(value)?
        .as_array()
        .ok_or(Error::syntax("numeric array expected"))?;
    let mut out = Vec::new();
    out.try_reserve_exact(values.len())
        .map_err(|_| Error::limit("numeric array allocation failed"))?;
    for value in values {
        out.push(number(document, value)?);
    }
    Ok(out)
}
