//! Mixed-byte and trimmed Unicode cmap layouts, pending OpenType primary review.
use super::word;
use crate::budget::Work;
use crate::read::{bad, here};
use crate::repairs;
use pagura_core::bytes::Cursor;
use pagura_core::convert::u32_to_usize;
use pagura_core::{Position, Result};

pub(super) fn validate_two(mut r: Cursor<'_>, glyphs: usize, work: &mut Work<'_>) -> Result<()> {
    r.skip(6)?;
    let mut largest = 0;
    for _ in 0..256 {
        let key = usize::from(r.u16()?);
        if key % 8 != 0 {
            return Err(bad(here(&r), "unaligned cmap subheader key"));
        }
        largest = largest.max(key / 8);
    }
    work.entries(largest + 1, Position::Buffer(here(&r)))?;
    work.work(256, Position::Buffer(here(&r)))?;
    let headers = super::available(&r, largest + 1, 8, work)?;
    for index in 0..headers {
        let mut header = r.cursor_at(518 + 8 * index)?;
        let first = usize::from(header.u16()?);
        let count = usize::from(header.u16()?);
        header.u16()?;
        let relative = usize::from(header.u16()?);
        if first + count > 256 || relative % 2 != 0 {
            return Err(bad(here(&header), "invalid cmap subheader range"));
        }
        work.work(count, Position::Buffer(here(&header)))?;
        let start = 524 + 8 * index + relative;
        if start > r.data().len() {
            work.repair(
                repairs::CMAP_TRUNCATED,
                "missing cmap subheader glyph array",
                Position::Buffer(here(&header) - 2),
            )?;
            continue;
        }
        let mut glyph_array = r.cursor_at(start)?;
        let count = super::available(&glyph_array, count, 2, work)?;
        for _ in 0..count {
            let raw = glyph_array.u16()?;
            let delta = word(r.data(), 522 + 8 * index)?;
            let glyph = if raw == 0 { 0 } else { raw.wrapping_add(delta) };
            if usize::from(glyph) >= glyphs {
                return Err(bad(here(&glyph_array), "invalid cmap glyph"));
            }
        }
    }
    Ok(())
}

pub(super) fn lookup_two(data: &[u8], code: u32) -> Result<u16> {
    if code > 65535 {
        return Ok(0);
    }
    let high =
        usize::try_from(code >> 8).map_err(|_| crate::read::limit(0, "cmap code overflow"))?;
    let low =
        usize::try_from(code & 255).map_err(|_| crate::read::limit(0, "cmap code overflow"))?;
    let key = usize::from(word(data, 6 + 2 * if high == 0 { low } else { high })?);
    if high == 0 && key != 0 || high != 0 && key == 0 {
        return Ok(0);
    }
    let at = 518 + if high == 0 { 0 } else { key };
    let first = usize::from(word(data, at)?);
    let count = usize::from(word(data, at + 2)?);
    if low < first || low - first >= count {
        return Ok(0);
    }
    let delta = word(data, at + 4)?;
    let relative = usize::from(word(data, at + 6)?);
    let glyph = word(data, at + 6 + relative + 2 * (low - first))?;
    Ok(if glyph == 0 {
        0
    } else {
        glyph.wrapping_add(delta)
    })
}

pub(super) fn validate_ten(mut r: Cursor<'_>, glyphs: usize, work: &mut Work<'_>) -> Result<()> {
    r.skip(12)?;
    let first = r.u32()?;
    let count = u32_to_usize(r.u32()?);
    if u64::from(first) + count as u64 > 0x0011_0000 {
        return Err(bad(here(&r), "invalid cmap Unicode range"));
    }
    work.entries(count, Position::Buffer(here(&r)))?;
    work.work(count, Position::Buffer(here(&r)))?;
    let count = super::available(&r, count, 2, work)?;
    for _ in 0..count {
        if usize::from(r.u16()?) >= glyphs {
            return Err(bad(here(&r), "invalid cmap glyph"));
        }
    }
    Ok(())
}
