//! Operand count and type checks of Annex A, Table A.1: a wrong count is
//! reported before any wrong type.

use super::{Dash, Properties};
use pagura_core::{Error, Name, Result, Value};

fn count(operands: &[Value], expected: usize) -> Result<()> {
    if operands.len() == expected {
        Ok(())
    } else {
        Err(Error::syntax("wrong operand count"))
    }
}

fn wrong_type() -> Error {
    Error::syntax("wrong operand type")
}

/// No operands; `value` is the result.
pub(super) fn none<T>(operands: &[Value], value: T) -> Result<T> {
    count(operands, 0).map(|()| value)
}

/// `N` numbers.
pub(super) fn numbers<const N: usize>(operands: &[Value]) -> Result<[f64; N]> {
    count(operands, N)?;
    let mut out = [0.0; N];
    for (out, value) in out.iter_mut().zip(operands) {
        *out = value.as_number().ok_or_else(wrong_type)?;
    }
    Ok(out)
}

/// One integer.
pub(super) fn integer(operands: &[Value]) -> Result<i64> {
    count(operands, 1)?;
    operands
        .first()
        .and_then(Value::as_integer)
        .ok_or_else(wrong_type)
}

/// One name, taken from `operands`.
pub(super) fn name(operands: &mut Vec<Value>) -> Result<Name> {
    count(operands, 1)?;
    match operands.pop() {
        Some(Value::Name(name)) => Ok(name),
        _ => Err(wrong_type()),
    }
}

/// One string, taken from `operands`.
pub(super) fn string(operands: &mut Vec<Value>) -> Result<Vec<u8>> {
    count(operands, 1)?;
    match operands.pop() {
        Some(Value::String(bytes)) => Ok(bytes),
        _ => Err(wrong_type()),
    }
}

/// One array, taken from `operands`.
pub(super) fn array(operands: &mut Vec<Value>) -> Result<Vec<Value>> {
    count(operands, 1)?;
    match operands.pop() {
        Some(Value::Array(values)) => Ok(values),
        _ => Err(wrong_type()),
    }
}

/// A name and a number (`Tf`), taken from `operands`.
pub(super) fn name_number(operands: &mut Vec<Value>) -> Result<(Name, f64)> {
    count(operands, 2)?;
    let size = operands.pop().and_then(|v| v.as_number());
    match (operands.pop(), size) {
        (Some(Value::Name(name)), Some(size)) => Ok((name, size)),
        _ => Err(wrong_type()),
    }
}

/// A tag and a properties name or dictionary (`BDC`, `DP`), taken from
/// `operands`.
pub(super) fn marked(operands: &mut Vec<Value>) -> Result<(Name, Properties)> {
    count(operands, 2)?;
    let properties = operands.pop();
    match (operands.pop(), properties) {
        (Some(Value::Name(tag)), Some(Value::Name(name))) => Ok((tag, Properties::Name(name))),
        (Some(Value::Name(tag)), Some(Value::Dictionary(d))) => Ok((tag, Properties::Inline(d))),
        _ => Err(wrong_type()),
    }
}

/// Two numbers and a string (`"`), taken from `operands`.
pub(super) fn spaced_string(operands: &mut Vec<Value>) -> Result<(f64, f64, Vec<u8>)> {
    count(operands, 3)?;
    let text = operands.pop();
    let character = operands.pop().and_then(|v| v.as_number());
    let word = operands.pop().and_then(|v| v.as_number());
    match (word, character, text) {
        (Some(word), Some(character), Some(Value::String(text))) => Ok((word, character, text)),
        _ => Err(wrong_type()),
    }
}

/// A dash array and phase (`d`).
pub(super) fn dash(operands: &[Value]) -> Result<Dash> {
    count(operands, 2)?;
    match operands {
        [Value::Array(array), phase] => {
            let phase = phase.as_number().ok_or_else(wrong_type)?;
            Dash::new(array, phase)
        }
        _ => Err(wrong_type()),
    }
}
