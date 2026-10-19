//! FDSelect 0/3 (TN 5176 section 19); format 4 awaits OpenType primary review.
use super::Version;
use crate::budget::Work;
use crate::read::limit;
use pagura_core::Result;
use pagura_core::bytes::Cursor;
use pagura_core::convert::u32_to_usize;

/// FDSelect formats: one FD per glyph, or ranges with 16-bit (format 3,
/// CFF) or 32-bit (format 4, CFF2) glyph boundaries.
const PER_GLYPH: u8 = 0;
const RANGES: u8 = 3;
const WIDE_RANGES: u8 = 4;

/// The FD index of every glyph, each below `fonts`.
pub(super) fn parse(
    mut r: Cursor<'_>,
    glyphs: usize,
    fonts: usize,
    version: Version,
    work: &mut Work<'_>,
) -> Result<Vec<u16>> {
    work.entries(glyphs, r.position())?;
    let format = r.u8()?;
    let mut result = Vec::new();
    result
        .try_reserve_exact(glyphs)
        .map_err(|_| limit(r.position(), "FDSelect allocation failed"))?;
    match format {
        PER_GLYPH => {
            for _ in 0..glyphs {
                result.push(u16::from(r.u8()?));
            }
        }
        RANGES => ranges(&mut r, Width::Short, glyphs, &mut result, work)?,
        WIDE_RANGES if version == Version::Cff2 => {
            ranges(&mut r, Width::Wide, glyphs, &mut result, work)?;
        }
        _ => return Err(r.error("invalid FDSelect format")),
    }
    if result.iter().any(|&fd| usize::from(fd) >= fonts) {
        return Err(r.error("FDSelect index outside FDArray"));
    }
    Ok(result)
}

/// The field widths of a range format.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Width {
    /// Format 3: 16-bit glyph boundaries and 8-bit FD indices.
    Short,
    /// Format 4: 32-bit glyph boundaries and 16-bit FD indices.
    Wide,
}

fn ranges(
    r: &mut Cursor<'_>,
    width: Width,
    glyphs: usize,
    result: &mut Vec<u16>,
    work: &mut Work<'_>,
) -> Result<()> {
    let count = boundary(r, width)?;
    if count == 0 || count > glyphs {
        return Err(r.error("invalid FDSelect range count"));
    }
    work.work(count.saturating_add(glyphs), r.position())?;
    let mut first = boundary(r, width)?;
    if first != 0 {
        return Err(r.error("FDSelect must start at glyph zero"));
    }
    for _ in 0..count {
        let fd = match width {
            Width::Short => u16::from(r.u8()?),
            Width::Wide => r.u16()?,
        };
        let next = boundary(r, width)?;
        if next <= first || next > glyphs {
            return Err(r.error("invalid FDSelect range"));
        }
        result.resize(next, fd);
        first = next;
    }
    if first != glyphs {
        return Err(r.error("invalid FDSelect sentinel"));
    }
    Ok(())
}

fn boundary(r: &mut Cursor<'_>, width: Width) -> Result<usize> {
    match width {
        Width::Short => Ok(usize::from(r.u16()?)),
        Width::Wide => Ok(u32_to_usize(r.u32()?)),
    }
}
