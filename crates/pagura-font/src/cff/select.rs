//! FDSelect 0/3 (TN 5176 section 19); format 4 awaits OpenType primary review.
use crate::read::{bad, cap, here};
use pagura_core::bytes::Cursor;
use pagura_core::convert::u32_to_usize;
use pagura_core::{Limits, Result};

pub(super) fn parse(
    mut r: Cursor<'_>,
    glyphs: usize,
    fonts: usize,
    limits: &Limits,
    cff2: bool,
) -> Result<Vec<usize>> {
    cap(
        glyphs,
        limits
            .value_nodes
            .min(limits.decoded_bytes / size_of::<usize>()),
        here(&r),
    )?;
    let format = r.u8()?;
    let mut result = Vec::with_capacity(glyphs);
    match format {
        0 => {
            for _ in 0..glyphs {
                result.push(usize::from(r.u8()?));
            }
        }
        3 | 4 if format == 3 || cff2 => ranges(&mut r, format, glyphs, &mut result, limits)?,
        _ => return Err(bad(here(&r), "invalid FDSelect format")),
    }
    if result.iter().any(|&fd| fd >= fonts) {
        return Err(bad(here(&r), "FDSelect index outside FDArray"));
    }
    Ok(result)
}
fn ranges(
    r: &mut Cursor<'_>,
    format: u8,
    glyphs: usize,
    result: &mut Vec<usize>,
    limits: &Limits,
) -> Result<()> {
    let count = boundary(r, format)?;
    if count == 0 || count > glyphs {
        return Err(bad(here(r), "invalid FDSelect range count"));
    }
    cap(count.saturating_add(glyphs), limits.operations, here(r))?;
    let mut first = boundary(r, format)?;
    if first != 0 {
        return Err(bad(here(r), "FDSelect must start at glyph zero"));
    }
    for _ in 0..count {
        let fd = if format == 4 {
            usize::from(r.u16()?)
        } else {
            usize::from(r.u8()?)
        };
        let next = boundary(r, format)?;
        if next <= first || next > glyphs {
            return Err(bad(here(r), "invalid FDSelect range"));
        }
        result.resize(next, fd);
        first = next;
    }
    if first != glyphs {
        return Err(bad(here(r), "invalid FDSelect sentinel"));
    }
    Ok(())
}
fn boundary(r: &mut Cursor<'_>, format: u8) -> Result<usize> {
    if format == 4 {
        Ok(u32_to_usize(r.u32()?))
    } else {
        Ok(usize::from(r.u16()?))
    }
}
