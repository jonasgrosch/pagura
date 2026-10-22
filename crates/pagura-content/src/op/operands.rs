//! Operand count and type checks of Annex A, Table A.1: a wrong count is
//! reported before any wrong type.

use super::{Buffers, Dash, Item, Operand, Properties, Slot};
use pagura_core::{Error, Name, Result, Value};

fn count(operands: &[Slot], expected: usize) -> Result<()> {
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
pub(super) fn none<T>(operands: &[Slot], value: T) -> Result<T> {
    count(operands, 0).map(|()| value)
}

/// `N` numbers.
pub(super) fn numbers<const N: usize>(operands: &[Slot]) -> Result<[f64; N]> {
    count(operands, N)?;
    let mut out = [0.0; N];
    for (out, slot) in out.iter_mut().zip(operands) {
        *out = slot.operand.number().ok_or_else(wrong_type)?;
    }
    Ok(out)
}

/// One integer.
pub(super) fn integer(operands: &[Slot]) -> Result<i64> {
    count(operands, 1)?;
    operands
        .first()
        .and_then(|slot| slot.operand.integer())
        .ok_or_else(wrong_type)
}

/// One name, taken from `operands`.
pub(super) fn name(operands: &mut [Slot]) -> Result<Name> {
    count(operands, 1)?;
    take_name(operands.first_mut())
}

fn take_name(slot: Option<&mut Slot>) -> Result<Name> {
    slot.and_then(|slot| slot.operand.take_name())
        .ok_or_else(wrong_type)
}

/// The bytes of a string operand.
fn bytes<'a>(slot: &Slot, buffers: &'a Buffers) -> Option<&'a [u8]> {
    match &slot.operand {
        Operand::String(span) => buffers.bytes.get(span.clone()),
        _ => None,
    }
}

/// One string.
pub(super) fn string<'a>(operands: &[Slot], buffers: &'a Buffers) -> Result<&'a [u8]> {
    count(operands, 1)?;
    operands
        .first()
        .and_then(|slot| bytes(slot, buffers))
        .ok_or_else(wrong_type)
}

/// One array: read in place or by the object parser.
pub(super) fn array(operands: &[Slot]) -> Result<&Operand> {
    count(operands, 1)?;
    match operands.first().map(|slot| &slot.operand) {
        Some(operand @ (Operand::Array(_) | Operand::Value(Value::Array(_)))) => Ok(operand),
        _ => Err(wrong_type()),
    }
}

/// A name and a number (`Tf`), the name taken from `operands`.
pub(super) fn name_number(operands: &mut [Slot]) -> Result<(Name, f64)> {
    count(operands, 2)?;
    let size = operands.get(1).and_then(|slot| slot.operand.number());
    match (take_name(operands.first_mut()), size) {
        (Ok(name), Some(size)) => Ok((name, size)),
        _ => Err(wrong_type()),
    }
}

/// A tag and a properties name or dictionary (`BDC`, `DP`), taken from
/// `operands`.
pub(super) fn marked(operands: &mut [Slot]) -> Result<(Name, Properties)> {
    count(operands, 2)?;
    let null = || Operand::Value(Value::Null);
    let mut take = |index: usize| {
        operands
            .get_mut(index)
            .map_or_else(null, |slot| std::mem::replace(&mut slot.operand, null()))
    };
    let (tag, properties) = (take(0), take(1));
    match (tag, properties) {
        (Operand::Name(tag), Operand::Name(name)) => Ok((tag, Properties::Name(name))),
        (Operand::Name(tag), Operand::Value(Value::Dictionary(d))) => {
            Ok((tag, Properties::Inline(d)))
        }
        _ => Err(wrong_type()),
    }
}

/// Two numbers and a string (`"`).
pub(super) fn spaced_string<'a>(
    operands: &[Slot],
    buffers: &'a Buffers,
) -> Result<(f64, f64, &'a [u8])> {
    count(operands, 3)?;
    match operands {
        [word, character, text] => match (
            word.operand.number(),
            character.operand.number(),
            bytes(text, buffers),
        ) {
            (Some(word), Some(character), Some(text)) => Ok((word, character, text)),
            _ => Err(wrong_type()),
        },
        _ => Err(wrong_type()),
    }
}

/// A dash array and phase (`d`).
pub(super) fn dash(operands: &[Slot], buffers: &Buffers) -> Result<Dash> {
    count(operands, 2)?;
    let [array, phase] = operands else {
        return Err(wrong_type());
    };
    let phase = match &array.operand {
        Operand::Array(_) | Operand::Value(Value::Array(_)) => {
            phase.operand.number().ok_or_else(wrong_type)?
        }
        _ => return Err(wrong_type()),
    };
    match &array.operand {
        Operand::Array(span) => {
            let items = buffers.items.get(span.clone()).unwrap_or_default();
            Dash::from_entries(
                items.iter().map(|item| match item {
                    Item::Number(n) => Some(*n),
                    Item::Show(_) => None,
                }),
                phase,
            )
        }
        operand => Dash::new(operand.values(), phase),
    }
}
