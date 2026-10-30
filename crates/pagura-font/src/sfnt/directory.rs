//! Bounded sfnt directory recovery; 9.9.1/Table 124 and the OpenType table
//! directory.
use super::Tables;
use crate::budget::Work;
use crate::read::{cursor, here, limit, syntax};
use crate::repairs;
use pagura_core::bytes::Cursor;
use pagura_core::convert::u32_to_usize;
use pagura_core::{Position, Result};
use std::ops::Range;

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

/// The same directory reader serves selected-face parsing and name lookup.
pub(super) struct Records<'a> {
    pub(super) signature: u32,
    pub(super) range: Range<usize>,
    cursor: Cursor<'a>,
}

pub(super) struct Record {
    pub(super) tag: [u8; 4],
    pub(super) range: Range<usize>,
    pub(super) position: Position,
    length_position: Position,
}

pub(super) fn records<'a>(
    data: &'a [u8],
    offset: usize,
    work: &mut Work<'_>,
) -> Result<Records<'a>> {
    let mut r = cursor(data).cursor_at(offset)?;
    let signature = r.u32()?;
    let at = r.position();
    let count = usize::from(r.u16()?);
    work.work(count + 1, at)?;
    r.skip(6)?; // searchRange, entrySelector, rangeShift
    let records = r.take_cursor(TABLE_RECORD * count)?;
    Ok(Records {
        signature,
        range: offset..here(&r),
        cursor: records,
    })
}

impl Records<'_> {
    pub(super) fn next(&mut self) -> Result<Option<Record>> {
        if self.cursor.at_end() {
            return Ok(None);
        }
        let r = &mut self.cursor;
        let tag = r.take_array::<4>()?;
        r.skip(4)?; // checksum
        let position = r.position();
        let start = u32_to_usize(r.u32()?);
        let length_position = r.position();
        let length = u32_to_usize(r.u32()?);
        let end = start
            .checked_add(length)
            .ok_or_else(|| limit(position, "table size overflow"))?;
        Ok(Some(Record {
            tag,
            range: start..end,
            position,
            length_position,
        }))
    }
}

impl Record {
    pub(super) fn check_overlap(&self, directory: &Range<usize>, header_end: usize) -> Result<()> {
        if !self.range.is_empty()
            && (self.range.start < header_end
                || (self.range.start < directory.end && directory.start < self.range.end))
        {
            return Err(syntax(self.position, "sfnt table overlaps directory"));
        }
        Ok(())
    }
}

pub(super) fn parse(
    data: &[u8],
    offset: usize,
    header_end: usize,
    work: &mut Work<'_>,
) -> Result<Directory> {
    let mut records = records(data, offset, work)?;
    work.entries(
        (records.range.len() - OFFSET_TABLE) / TABLE_RECORD,
        Position::Buffer(offset),
    )?;
    let mut tables = Tables::new();
    let mut clipped_post = false;
    while let Some(record) = records.next()? {
        work.decoded(record.range.len(), record.length_position)?;
        record.check_overlap(&records.range, header_end)?;
        if !record.range.is_empty() && !record.range.start.is_multiple_of(TABLE_ALIGNMENT) {
            work.repair(
                repairs::TABLE_ALIGNMENT,
                "unaligned sfnt table offset",
                record.position,
            )?;
        }
        if record.range.end > data.len() {
            clipped_post |= &record.tag == b"post";
            work.repair(
                repairs::TABLE_BOUNDS,
                "sfnt table exceeds available data",
                record.position,
            )?;
        }
        let range = record.range.start.min(data.len())..record.range.end.min(data.len());
        if tables.insert(record.tag, range).is_some() {
            return Err(records.cursor.error("duplicate sfnt table"));
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
    Ok(Directory {
        flavor: flavor(records.signature, &tables, Position::Buffer(offset), work)?,
        tables,
        clipped_post,
    })
}

fn flavor(signature: u32, tables: &Tables, at: Position, work: &mut Work<'_>) -> Result<Flavor> {
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
        return Err(syntax(at, "unsupported sfnt version"));
    }
    work.repair(
        repairs::SFNT_VERSION,
        "unknown sfnt version inferred from required tables",
        at,
    )?;
    Ok(if cff { Flavor::Cff } else { Flavor::TrueType })
}
