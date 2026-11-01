//! Bounded sfnt directory recovery; 9.9.1/Table 124 and the OpenType table
//! directory.
mod count;

use super::{DIRECTORY, Tables};
use crate::budget::Work;
use crate::read::{cursor, here, limit, syntax};
use crate::repairs;
use pagura_core::convert::u32_to_usize;
use pagura_core::{ErrorKind, Position, Recovery, Result};

/// sfnt versions: TrueType outlines, CFF outlines (`OTTO`), and Apple's
/// original tag for the TrueType container (`true`).
const TRUETYPE: u32 = 0x0001_0000;
const OTTO: u32 = u32::from_be_bytes(*b"OTTO");
const APPLE_TRUE: u32 = u32::from_be_bytes(*b"true");
/// The offset table and each table record (OpenType table directory).
const OFFSET_TABLE: usize = 12;
const TABLE_RECORD: usize = 16;
/// Tables start on four-byte boundaries.
const TABLE_ALIGNMENT: usize = 4;
/// The tables that make up a TrueType-outline font (9.9.1, Table 124).
const GLYF_TABLES: [&[u8; 4]; 6] = [b"glyf", b"head", b"hhea", b"hmtx", b"loca", b"maxp"];

/// The outline technology an sfnt container declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Flavor {
    /// `glyf` outlines.
    TrueType,
    /// `CFF ` or `CFF2` outlines.
    Cff,
}

pub(super) struct Directory {
    pub(super) flavor: Flavor,
    pub(super) tables: Tables,
    /// The `post` table was clamped to the available data.
    pub(super) clipped_post: bool,
}

pub(super) fn parse(data: &[u8], work: &mut Work<'_>) -> Result<Directory> {
    let mut r = cursor(data);
    let signature = r.u32()?;
    let at = r.position();
    let count = usize::from(r.u16()?);
    work.entries(count, at)?;
    r.skip(6)?; // searchRange, entrySelector, rangeShift
    let parsed = read_tables(data, count, work);
    let (tables, clipped_post) = match parsed {
        Ok(value) => value,
        Err(error)
            if error.kind == ErrorKind::Syntax
                && error.message == "sfnt table overlaps directory"
                && work.mode() == Recovery::Repair =>
        {
            let Some(actual) = count::prove(data, signature, count, work)? else {
                return Err(error);
            };
            work.repair(
                repairs::SFNT_DIRECTORY_COUNT,
                "sfnt directory count includes the first table header",
                at,
            )?;
            // The original declared entry charge covers this smaller directory.
            // The failed attempt's table map has already been dropped.
            read_tables(data, actual, work)?
        }
        Err(error) => return Err(error),
    };
    Ok(Directory {
        flavor: flavor(signature, &tables, work)?,
        tables,
        clipped_post,
    })
}

fn read_tables(data: &[u8], count: usize, work: &mut Work<'_>) -> Result<(Tables, bool)> {
    let mut r = cursor(data);
    r.skip(OFFSET_TABLE)?;
    // A u16 count times sixteen plus twelve fits every supported usize.
    let directory_end = OFFSET_TABLE + TABLE_RECORD * count;
    let mut tables = Tables::new();
    let mut clipped_post = false;
    for _ in 0..count {
        let tag = r.take_array::<4>()?;
        r.skip(4)?; // checksum
        let record = Position::Buffer(here(&r));
        let start = u32_to_usize(r.u32()?);
        let len_at = r.position();
        let len = u32_to_usize(r.u32()?);
        work.decoded(len, len_at)?;
        if len != 0 && start < directory_end {
            return Err(syntax(record, "sfnt table overlaps directory"));
        }
        if len != 0 && !start.is_multiple_of(TABLE_ALIGNMENT) {
            work.repair(
                repairs::TABLE_ALIGNMENT,
                "unaligned sfnt table offset",
                record,
            )?;
        }
        let end = start
            .checked_add(len)
            .ok_or_else(|| limit(record, "table size overflow"))?;
        if end > data.len() {
            clipped_post |= &tag == b"post";
            work.repair(
                repairs::TABLE_BOUNDS,
                "sfnt table exceeds available data",
                record,
            )?;
        }
        let range = start.min(data.len())..end.min(data.len());
        if tables.insert(tag, range).is_some() {
            return Err(r.error("duplicate sfnt table"));
        }
    }
    let mut ranges: Vec<_> = tables.values().filter(|r| !r.is_empty()).collect();
    ranges.sort_by_key(|r| r.start);
    for pair in ranges.windows(2) {
        if let [a, b] = pair
            && a.end > b.start
        {
            return Err(syntax(Position::Buffer(b.start), "overlapping sfnt tables"));
        }
    }
    Ok((tables, clipped_post))
}

fn flavor(signature: u32, tables: &Tables, work: &mut Work<'_>) -> Result<Flavor> {
    match signature {
        // Apple's original TrueType tag names the same glyf container. It is
        // not a repaired unknown flavor. OpenType's primary text is pending.
        TRUETYPE | APPLE_TRUE => return Ok(Flavor::TrueType),
        OTTO => return Ok(Flavor::Cff),
        _ => {}
    }
    let cff = (tables.contains_key(b"CFF ") || tables.contains_key(b"CFF2"))
        && !tables.contains_key(b"glyf");
    let glyf = GLYF_TABLES.iter().all(|t| tables.contains_key(*t));
    if !cff && !glyf {
        return Err(syntax(DIRECTORY, "unsupported sfnt version"));
    }
    work.repair(
        repairs::SFNT_VERSION,
        "unknown sfnt version inferred from required tables",
        DIRECTORY,
    )?;
    Ok(if cff { Flavor::Cff } else { Flavor::TrueType })
}
