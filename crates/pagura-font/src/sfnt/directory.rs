//! Bounded sfnt directory recovery; ISO 32000-2 9.9.1/Table 124.
use super::Tables;
use crate::read::{Read, bad, cap};
use crate::{recovery::repair, tags};
use pagura_core::{Diagnostics, Limits, Position, Result};

pub(super) fn directory(
    data: &[u8],
    limits: &Limits,
    diagnostics: &mut Diagnostics,
) -> Result<(u32, Tables, bool)> {
    let mut r = Read::new(data, 0);
    let signature = r.u32()?;
    let count = usize::from(r.u16()?);
    cap(
        count,
        limits
            .objects
            .min(limits.operations)
            .min(limits.value_nodes),
        4,
    )?;
    r.skip(6)?;
    let directory_end = 12 + 16 * count;
    let mut tables = Tables::new();
    let mut clipped_post = false;
    for _ in 0..count {
        let tag: [u8; 4] = r
            .take(4)?
            .try_into()
            .map_err(|_| bad(r.at(), "invalid table tag"))?;
        r.skip(4)?;
        let start = r.offset()?;
        let len = r.offset()?;
        cap(len, limits.decoded_bytes, r.at() - 4)?;
        if len != 0 && start < directory_end {
            return Err(bad(r.at() - 8, "sfnt table overlaps directory"));
        }
        if len != 0 && start % 4 != 0 {
            repair(
                diagnostics,
                tags::TABLE_ALIGNMENT,
                "unaligned sfnt table offset",
                Position::Buffer(r.at() - 8),
            )?;
        }
        let end = start
            .checked_add(len)
            .ok_or_else(|| crate::read::limit(r.at() - 8, "table size overflow"))?;
        if end > data.len() {
            clipped_post |= &tag == b"post";
            repair(
                diagnostics,
                tags::TABLE_BOUNDS,
                "sfnt table exceeds available data",
                Position::Buffer(r.at() - 8),
            )?;
        }
        let range = start.min(data.len())..end.min(data.len());
        if tables.insert(tag, range).is_some() {
            return Err(bad(r.at(), "duplicate sfnt table"));
        }
    }
    let mut ranges: Vec<_> = tables.values().filter(|r| !r.is_empty()).collect();
    ranges.sort_by_key(|r| r.start);
    if ranges.windows(2).any(|w| w[0].end > w[1].start) {
        return Err(bad(0, "overlapping sfnt tables"));
    }
    Ok((
        flavor(signature, &tables, diagnostics)?,
        tables,
        clipped_post,
    ))
}

fn flavor(signature: u32, tables: &Tables, diagnostics: &mut Diagnostics) -> Result<u32> {
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
    repair(
        diagnostics,
        tags::SFNT_VERSION,
        "unknown sfnt version inferred from required tables",
        Position::Buffer(0),
    )?;
    Ok(if cff { 0x4f54_544f } else { 0x0001_0000 })
}
