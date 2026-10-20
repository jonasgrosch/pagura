//! Semantic object comparison without decoding or normalizing stream bytes.
use crate::{
    Document, IndirectObject,
    view::{Context, Source},
};
use pagura_core::{Dictionary, Result, Value};
// Xref streams contain both trailer semantics and encoding/index bookkeeping.
pub(super) const TRAILER_KEYS: &[&[u8]] = &[
    b"Size",
    b"Prev",
    b"XRefStm",
    b"Type",
    b"W",
    b"Index",
    b"Length",
    b"Filter",
    b"DecodeParms",
];
pub(super) fn equal(
    ctx: &mut Context<'_>,
    before: &Document,
    after: &Document,
    a: &IndirectObject,
    b: &IndirectObject,
) -> Result<bool> {
    if !equal_value(ctx, a.value(), b.value(), 0)? || a.is_stream() != b.is_stream() {
        return Ok(false);
    }
    if a.is_stream() {
        let a = before.raw_stream_data(a)?;
        let b = after.raw_stream_data(b)?;
        // Charge byte comparison work using fixed-size blocks; equal large
        // streams do not evade the operation's node budget.
        for (a, b) in a.chunks(64).zip(b.chunks(64)) {
            ctx.step(0, Source::default())?;
            if a != b {
                return Ok(false);
            }
        }
        return Ok(a.len() == b.len());
    }
    Ok(true)
}
fn equal_value(ctx: &mut Context<'_>, a: &Value, b: &Value, depth: usize) -> Result<bool> {
    ctx.step(depth, Source::default())?;
    match (a, b) {
        (Value::Dictionary(a), Value::Dictionary(b)) => dictionary_at(ctx, a, b, &[], depth + 1),
        (Value::Array(a), Value::Array(b)) => {
            if a.len() != b.len() {
                return Ok(false);
            }
            for (a, b) in a.iter().zip(b) {
                if !equal_value(ctx, a, b, depth + 1)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        _ => Ok(a == b),
    }
}
pub(super) fn same_dictionary(
    ctx: &mut Context<'_>,
    a: &Dictionary,
    b: &Dictionary,
    ignored: &[&[u8]],
) -> Result<bool> {
    dictionary_at(ctx, a, b, ignored, 0)
}
fn dictionary_at(
    ctx: &mut Context<'_>,
    a: &Dictionary,
    b: &Dictionary,
    ignored: &[&[u8]],
    depth: usize,
) -> Result<bool> {
    for (key, value) in a {
        if ignored.contains(&key.as_bytes()) {
            continue;
        }
        let Some(other) = b.get(key.as_bytes()) else {
            return Ok(false);
        };
        if !equal_value(ctx, value, other, depth + 1)? {
            return Ok(false);
        }
    }
    Ok(b.iter()
        .all(|(key, _)| ignored.contains(&key.as_bytes()) || a.contains_key(key.as_bytes())))
}
