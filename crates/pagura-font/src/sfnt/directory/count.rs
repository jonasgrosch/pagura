//! Exact one-record directory-count excess, OpenType TableDirectory/TableRecord.
//! A claimed record that is really the first head/CFF header never supplies a
//! table: all offsets, extents and payloads come from the intact record prefix.
use super::{GLYF_TABLES, OFFSET_TABLE, OTTO, TABLE_RECORD, TRUETYPE};
use crate::budget::Work;
use crate::read::cursor;
use pagura_core::convert::u32_to_usize;
use pagura_core::{Position, Result};

const HEAD_BYTES: usize = 54;
const HEAD_MAGIC_OFFSET: usize = 12;
const HEAD_MAGIC: u32 = 0x5f0f_3cf5;
const CFF_HEADER_BYTES: usize = 4;
const CFF_MAJOR: u8 = 1;
const CFF_MINOR: u8 = 0;

#[derive(Clone, Copy)]
struct Record {
    tag: [u8; 4],
    start: usize,
    length: usize,
    end: usize,
}

impl Record {
    fn read(data: &[u8], index: usize) -> Option<Self> {
        let offset = directory_end(index)?;
        let end = offset.checked_add(TABLE_RECORD)?;
        let bytes = data.get(offset..end)?;
        let mut r = cursor(bytes);
        let tag = r.take_array::<4>().ok()?;
        r.skip(4).ok()?; // checksum
        let start = u32_to_usize(r.u32().ok()?);
        let length = u32_to_usize(r.u32().ok()?);
        Some(Self {
            tag,
            start,
            length,
            end: start.checked_add(length)?,
        })
    }

    fn overlaps(self, other: Self) -> bool {
        self.length != 0 && other.length != 0 && self.start < other.end && other.start < self.end
    }
}

fn directory_end(count: usize) -> Option<usize> {
    count.checked_mul(TABLE_RECORD)?.checked_add(OFFSET_TABLE)
}

fn tag_is_valid(tag: [u8; 4]) -> bool {
    tag.into_iter().all(|byte| (b' '..=b'~').contains(&byte))
}

/// A constant-storage proof; every record/comparison is charged before reading.
/// Source faults decline the proof and preserve the original overlap error.
/// A caller work Limit propagates and cannot become a failed proof/retry.
pub(super) fn prove(
    data: &[u8],
    signature: u32,
    declared: usize,
    work: &mut Work<'_>,
) -> Result<Option<usize>> {
    if !matches!(signature, TRUETYPE | OTTO) {
        return Ok(None);
    }
    let Some(actual) = declared.checked_sub(1).filter(|&n| n != 0) else {
        return Ok(None);
    };
    let Some(boundary) = directory_end(actual) else {
        return Ok(None);
    };
    let mut first: Option<Record> = None;
    let mut glyf = [false; GLYF_TABLES.len()];
    let mut cff = false;
    for index in 0..actual {
        work.work(1, Position::Buffer(OFFSET_TABLE))?;
        let Some(record) = Record::read(data, index) else {
            return Ok(None);
        };
        if !tag_is_valid(record.tag)
            || record.end > data.len()
            || record.start > data.len()
            || (record.length != 0 && record.start < boundary)
        {
            return Ok(None);
        }
        for previous in 0..index {
            work.work(1, Position::Buffer(OFFSET_TABLE))?;
            let Some(before) = Record::read(data, previous) else {
                return Ok(None);
            };
            if before.tag >= record.tag || before.overlaps(record) {
                return Ok(None);
            }
        }
        if record.length != 0 && first.is_none_or(|old| record.start < old.start) {
            first = Some(record);
        }
        for (present, tag) in glyf.iter_mut().zip(GLYF_TABLES) {
            *present |= record.tag == *tag;
        }
        cff |= record.tag == *b"CFF ";
        // CFF2 and mixed outline technologies need their own primary proof.
        if record.tag == *b"CFF2" || (signature == OTTO && record.tag == *b"glyf") {
            return Ok(None);
        }
    }
    let Some(first) = first.filter(|first| first.start == boundary) else {
        return Ok(None);
    };
    // The alleged extra record must have a full wire extent, and its tag is
    // the binary header of the first payload, not a valid additional table.
    let Some(end) = boundary.checked_add(TABLE_RECORD) else {
        return Ok(None);
    };
    let Some(extra) = data.get(boundary..end) else {
        return Ok(None);
    };
    work.work(1, Position::Buffer(boundary))?;
    let mut r = cursor(extra);
    if tag_is_valid(r.take_array::<4>()?) {
        return Ok(None);
    }
    let Some(payload) = data.get(first.start..first.end) else {
        return Ok(None);
    };
    let mut r = cursor(payload);
    let supported = if signature == TRUETYPE {
        !cff && glyf.into_iter().all(|present| present)
            && first.tag == *b"head"
            && payload.len() >= HEAD_BYTES
            && r.u32()? == TRUETYPE
            && r.cursor_at(HEAD_MAGIC_OFFSET)?.u32()? == HEAD_MAGIC
    } else {
        if !cff || first.tag != *b"CFF " || payload.len() < CFF_HEADER_BYTES {
            return Ok(None);
        }
        let major = r.u8()?;
        let minor = r.u8()?;
        let header = usize::from(r.u8()?);
        let offset_size = r.u8()?;
        major == CFF_MAJOR
            && minor == CFF_MINOR
            && (CFF_HEADER_BYTES..=payload.len()).contains(&header)
            && matches!(offset_size, 1..=4)
    };
    Ok(supported.then_some(actual))
}
