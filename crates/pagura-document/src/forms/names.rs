//! Bounded duplicate-value comparison, including indirect subvalues.
use crate::view::{Context, Source, identity};
use pagura_core::{Result, Value};
use std::collections::BTreeSet;

/// How two strings in compared values are equal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Strings {
    /// Byte for byte.
    Bytes,
    /// As decoded text strings (7.9.2.2), for text and choice field values.
    Text,
}

pub(super) fn equivalent<'a>(
    context: &mut Context<'a>,
    a: Option<&'a Value>,
    b: Option<&'a Value>,
    source: Source,
    strings: Strings,
) -> Result<bool> {
    let (Some(a), Some(b)) = (a, b) else {
        return Ok(a.is_none() && b.is_none());
    };
    let mut stack = vec![(a, b, 0)];
    let mut seen = BTreeSet::new();
    while let Some((a, b, depth)) = stack.pop() {
        context.step(depth, source)?;
        let (a, _) = context.resolve(a, source)?;
        let (b, _) = context.resolve(b, source)?;
        if std::ptr::eq(a, b) {
            continue;
        }
        context.index_entry(source)?;
        if !seen.insert((identity(a), identity(b))) {
            continue;
        }
        match (a, b) {
            (Value::Array(a), Value::Array(b)) => {
                if a.len() != b.len() {
                    return Ok(false);
                }
                context.allocate::<(&Value, &Value, usize)>(a.len(), source)?;
                stack.extend(a.iter().zip(b).map(|(a, b)| (a, b, depth + 1)));
            }
            (Value::Dictionary(a), Value::Dictionary(b)) => {
                if a.len() != b.len() {
                    return Ok(false);
                }
                context.allocate::<(&Value, &Value, usize)>(a.len(), source)?;
                for (key, a) in a {
                    let Some(b) = b.get(key.as_bytes()) else {
                        return Ok(false);
                    };
                    stack.push((a, b, depth + 1));
                }
            }
            (Value::String(a), Value::String(b)) => {
                let equal = match strings {
                    Strings::Text => {
                        let a = context.text(a, source)?;
                        let b = context.text(b, source)?;
                        bytes_equal(context, a.as_bytes(), b.as_bytes(), source)?
                    }
                    Strings::Bytes => bytes_equal(context, a, b, source)?,
                };
                if !equal {
                    return Ok(false);
                }
            }
            (Value::Name(a), Value::Name(b)) => {
                if !bytes_equal(context, a.as_bytes(), b.as_bytes(), source)? {
                    return Ok(false);
                }
            }
            _ if a != b => return Ok(false),
            _ => (),
        }
    }
    Ok(true)
}

fn bytes_equal(context: &mut Context<'_>, a: &[u8], b: &[u8], source: Source) -> Result<bool> {
    // Bound repeated comparisons of large shared source strings too: one
    // step per chunk.
    const CHUNK: usize = 64;
    if a.len() != b.len() {
        return Ok(false);
    }
    for (a, b) in a.chunks(CHUNK).zip(b.chunks(CHUNK)) {
        context.step(0, source)?;
        if a != b {
            return Ok(false);
        }
    }
    Ok(true)
}
