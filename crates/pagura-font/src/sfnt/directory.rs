//! Bounded sfnt directory recovery; ISO 32000-2 9.9.1/Table 124.
use super::Tables;
use crate::budget::Work;
use crate::read::{bad, cursor, here};
use crate::repairs;
use pagura_core::convert::u32_to_usize;
use pagura_core::{Position, Result};

pub(super) fn directory(data: &[u8], work: &mut Work<'_>) -> Result<(u32, Tables, bool)> {
    let mut r = cursor(data);
    let signature = r.u32()?;
    let count = usize::from(r.u16()?);
    work.entries(count, Position::Buffer(4))?;
    r.skip(6)?;
    let directory_end = 12 + 16 * count;
    let mut tables = Tables::new();
    let mut clipped_post = false;
    for _ in 0..count {
        let tag: [u8; 4] = r
            .take(4)?
            .try_into()
            .map_err(|_| bad(here(&r), "invalid table tag"))?;
        r.skip(4)?;
        let start = u32_to_usize(r.u32()?);
        let len = u32_to_usize(r.u32()?);
        work.decoded(len, Position::Buffer(here(&r) - 4))?;
        if len != 0 && start < directory_end {
            return Err(bad(here(&r) - 8, "sfnt table overlaps directory"));
        }
        if len != 0 && !start.is_multiple_of(4) {
            work.repair(
                repairs::TABLE_ALIGNMENT,
                "unaligned sfnt table offset",
                Position::Buffer(here(&r) - 8),
            )?;
        }
        let end = start
            .checked_add(len)
            .ok_or_else(|| crate::read::limit(here(&r) - 8, "table size overflow"))?;
        if end > data.len() {
            clipped_post |= &tag == b"post";
            work.repair(
                repairs::TABLE_BOUNDS,
                "sfnt table exceeds available data",
                Position::Buffer(here(&r) - 8),
            )?;
        }
        let range = start.min(data.len())..end.min(data.len());
        if tables.insert(tag, range).is_some() {
            return Err(bad(here(&r), "duplicate sfnt table"));
        }
    }
    let mut ranges: Vec<_> = tables.values().filter(|r| !r.is_empty()).collect();
    ranges.sort_by_key(|r| r.start);
    if ranges.windows(2).any(|w| w[0].end > w[1].start) {
        return Err(bad(0, "overlapping sfnt tables"));
    }
    Ok((flavor(signature, &tables, work)?, tables, clipped_post))
}

fn flavor(signature: u32, tables: &Tables, work: &mut Work<'_>) -> Result<u32> {
    match signature {
        0x0001_0000 | 0x4f54_544f => return Ok(signature),
        // Apple's original TrueType tag names the same glyf container. It is
        // not a repaired unknown flavor. OpenType's primary text is pending.
        0x7472_7565 => return Ok(0x0001_0000),
        _ => {}
    }
    let cff = (tables.contains_key(b"CFF ") || tables.contains_key(b"CFF2"))
        && !tables.contains_key(b"glyf");
    let glyf = [b"glyf", b"head", b"hhea", b"hmtx", b"loca", b"maxp"]
        .iter()
        .all(|t| tables.contains_key(*t));
    if !cff && !glyf {
        return Err(bad(0, "unsupported sfnt version"));
    }
    work.repair(
        repairs::SFNT_VERSION,
        "unknown sfnt version inferred from required tables",
        Position::Buffer(0),
    )?;
    Ok(if cff { 0x4f54_544f } else { 0x0001_0000 })
}
