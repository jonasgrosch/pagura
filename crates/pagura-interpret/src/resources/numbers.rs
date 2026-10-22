//! Bounded reads of numeric PDF arrays, shared by colour spaces, shadings,
//! patterns, functions and annotations.

use pagura_core::{Error, Result, Value};
use pagura_document::Document;

/// The magnitude above which geometric parameters (shading coordinates,
/// pattern matrices) leave the range where their arithmetic stays exact
/// enough to render.
pub(crate) const GEOMETRY: f64 = 1e12;

/// Resolves `value` to an array of exactly `N` numbers, each resolved and
/// at most `magnitude` in absolute value.
///
/// # Errors
/// `Syntax` for a non-array, a wrong length or a non-numeric entry, and
/// `Limit` for a number beyond `magnitude`.
pub(crate) fn array<const N: usize>(
    document: &Document,
    value: &Value,
    magnitude: f64,
) -> Result<[f64; N]> {
    let values = document
        .resolve(value)?
        .as_array()
        .filter(|v| v.len() == N)
        .ok_or(Error::syntax("numeric array has the wrong length"))?;
    let mut out = [0.0; N];
    for (out, value) in out.iter_mut().zip(values) {
        *out = number(document, value, magnitude)?;
    }
    Ok(out)
}

/// Resolves `value` to an array of numbers of any length, each at most
/// `magnitude` in absolute value. The array's length was bounded when the
/// document parsed it.
///
/// # Errors
/// As [`array`], except for the length.
pub(crate) fn list(document: &Document, value: &Value, magnitude: f64) -> Result<Vec<f64>> {
    let values = document
        .resolve(value)?
        .as_array()
        .ok_or(Error::syntax("numeric array expected"))?;
    let mut out = Vec::new();
    out.try_reserve_exact(values.len())
        .map_err(|_| Error::limit("numeric array allocation failed"))?;
    for value in values {
        out.push(number(document, value, magnitude)?);
    }
    Ok(out)
}

/// Resolves one number, at most `magnitude` in absolute value.
///
/// # Errors
/// As [`array`].
pub(crate) fn number(document: &Document, value: &Value, magnitude: f64) -> Result<f64> {
    let number = document
        .resolve(value)?
        .as_number()
        .ok_or(Error::syntax("array entry must be a number"))?;
    if !number.is_finite() || number.abs() > magnitude {
        return Err(Error::limit("number exceeds numeric working range"));
    }
    Ok(number)
}

/// Resolves an optional dictionary entry.
///
/// # Errors
/// Errors from resolving the value.
pub(crate) fn entry<'a>(
    document: &'a Document,
    dictionary: &'a pagura_core::Dictionary,
    key: &[u8],
) -> Result<Option<&'a Value>> {
    dictionary
        .get(key)
        .map(|value| document.resolve(value))
        .transpose()
}
