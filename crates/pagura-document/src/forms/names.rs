//! Bounded duplicate-value comparison, including indirect subvalues.
use crate::navigation::context::{Context, Source, identity};
use pagura_core::{Result, Value};
use std::collections::BTreeSet;

pub(super) fn equivalent<'a>(
    ctx: &mut Context<'a>,
    a: Option<&'a Value>,
    b: Option<&'a Value>,
    source: Source,
    text_strings: bool,
) -> Result<bool> {
    let (Some(a), Some(b)) = (a, b) else {
        return Ok(a.is_none() && b.is_none());
    };
    let mut stack = vec![(a, b, 0)];
    let mut seen = BTreeSet::new();
    while let Some((a, b, depth)) = stack.pop() {
        ctx.step(depth, source)?;
        let (a, _) = ctx.resolve(a, source)?;
        let (b, _) = ctx.resolve(b, source)?;
        if std::ptr::eq(a, b) {
            continue;
        }
        ctx.allocate::<usize>(8, source)?;
        if !seen.insert((identity(a), identity(b))) {
            continue;
        }
        match (a, b) {
            (Value::Array(a), Value::Array(b)) => {
                if a.len() != b.len() {
                    return Ok(false);
                }
                ctx.allocate::<(&Value, &Value, usize)>(a.len(), source)?;
                stack.extend(a.iter().zip(b).map(|(a, b)| (a, b, depth + 1)));
            }
            (Value::Dictionary(a), Value::Dictionary(b)) => {
                if a.len() != b.len() {
                    return Ok(false);
                }
                ctx.allocate::<(&Value, &Value, usize)>(a.len(), source)?;
                for (key, a) in a.iter() {
                    let Some(b) = b.get(key.as_bytes()) else {
                        return Ok(false);
                    };
                    stack.push((a, b, depth + 1));
                }
            }
            (Value::String(a), Value::String(b)) => {
                let equal = if text_strings {
                    let a = crate::navigation::text::decode(ctx, a, source)?;
                    let b = crate::navigation::text::decode(ctx, b, source)?;
                    bytes_equal(ctx, a.as_bytes(), b.as_bytes(), source)?
                } else {
                    bytes_equal(ctx, a, b, source)?
                };
                if !equal {
                    return Ok(false);
                }
            }
            (Value::Name(a), Value::Name(b)) => {
                if !bytes_equal(ctx, a.as_bytes(), b.as_bytes(), source)? {
                    return Ok(false);
                }
            }
            _ if a != b => return Ok(false),
            _ => (),
        }
    }
    Ok(true)
}

fn bytes_equal(ctx: &mut Context<'_>, a: &[u8], b: &[u8], source: Source) -> Result<bool> {
    if a.len() != b.len() {
        return Ok(false);
    }
    // Bound repeated comparisons of large shared source strings too.
    for (a, b) in a.chunks(64).zip(b.chunks(64)) {
        ctx.step(0, source)?;
        if a != b {
            return Ok(false);
        }
    }
    Ok(true)
}
