//! FDSelect formats 0, 3 and 4; external layouts pending primary-text review.
use crate::read::{Read, bad, cap};
use pagura_core::{Limits, Result};

pub(super) fn parse(
    mut r: Read<'_>,
    glyphs: usize,
    fonts: usize,
    limits: &Limits,
) -> Result<Vec<usize>> {
    cap(
        glyphs,
        limits
            .value_nodes
            .min(limits.decoded_bytes / size_of::<usize>()),
        r.at(),
    )?;
    let format = r.u8()?;
    let mut result = Vec::with_capacity(glyphs);
    match format {
        0 => {
            for _ in 0..glyphs {
                result.push(usize::from(r.u8()?));
            }
        }
        3 | 4 => ranges(&mut r, format, glyphs, &mut result, limits)?,
        _ => return Err(bad(r.at(), "invalid FDSelect format")),
    }
    if result.iter().any(|&fd| fd >= fonts) {
        return Err(bad(r.at(), "FDSelect index outside FDArray"));
    }
    Ok(result)
}
fn ranges(
    r: &mut Read<'_>,
    format: u8,
    glyphs: usize,
    result: &mut Vec<usize>,
    limits: &Limits,
) -> Result<()> {
    let count = boundary(r, format)?;
    if count == 0 || count > glyphs {
        return Err(bad(r.at(), "invalid FDSelect range count"));
    }
    cap(count.saturating_add(glyphs), limits.operations, r.at())?;
    let mut first = boundary(r, format)?;
    if first != 0 {
        return Err(bad(r.at(), "FDSelect must start at glyph zero"));
    }
    for _ in 0..count {
        let fd = if format == 4 {
            usize::from(r.u16()?)
        } else {
            usize::from(r.u8()?)
        };
        let next = boundary(r, format)?;
        if next <= first || next > glyphs {
            return Err(bad(r.at(), "invalid FDSelect range"));
        }
        result.resize(next, fd);
        first = next;
    }
    if first != glyphs {
        return Err(bad(r.at(), "invalid FDSelect sentinel"));
    }
    Ok(())
}
fn boundary(r: &mut Read<'_>, format: u8) -> Result<usize> {
    if format == 4 {
        r.offset()
    } else {
        Ok(usize::from(r.u16()?))
    }
}
