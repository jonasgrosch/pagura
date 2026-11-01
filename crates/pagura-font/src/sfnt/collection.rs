//! OpenType TTC Header and naming-table ID 6. Collection selection borrows
//! the original file; shared table offsets remain relative to that file.
use super::directory;
use crate::budget::Work;
use crate::read::{cursor, here, limit, slice, syntax};
use crate::{FontFace, features};
use pagura_core::bytes::Cursor;
use pagura_core::convert::u32_to_usize;
use pagura_core::{Error, Position, Result};

const COLLECTION: &[u8; 4] = b"ttcf";
const VERSION_1: u32 = 0x0001_0000;
const VERSION_2: u32 = 0x0002_0000;
const SIGNATURE: u32 = u32::from_be_bytes(*b"DSIG");
const NAME_RECORD: usize = 12;
const LANGUAGE_RECORD: usize = 4;
const POSTSCRIPT_NAME: u16 = 6;
const MAX_NAME: usize = 63;
const LANGUAGE_TAG: u16 = 0x8000;

pub(super) struct Selected {
    pub(super) offset: usize,
    pub(super) header_end: usize,
}

pub(super) fn select(data: &[u8], face: FontFace<'_>, work: &mut Work<'_>) -> Result<Selected> {
    if !data.starts_with(COLLECTION) {
        if matches!(face, FontFace::Index(index) if index != 0) {
            return Err(Error::invalid_argument(
                "standalone font index must be zero",
            ));
        }
        return Ok(Selected {
            offset: 0,
            header_end: 0,
        });
    }
    let mut r = cursor(data);
    r.skip(4)?;
    let at = r.position();
    let version = r.u32()?;
    if !matches!(version, VERSION_1 | VERSION_2) {
        return Err(Error::unsupported(features::FORMAT, "unknown font collection version").at(at));
    }
    let at = r.position();
    let count = u32_to_usize(r.u32()?);
    if count == 0 {
        return Err(syntax(at, "empty font collection"));
    }
    work.work(count, at)?;
    let bytes = count
        .checked_mul(4)
        .ok_or_else(|| limit(at, "collection directory size overflow"))?;
    let mut offsets = r.take_cursor(bytes)?;
    if version == VERSION_2 {
        signature(&mut r, data.len())?;
    }
    let header_end = here(&r);
    let offset = match face {
        FontFace::Index(index) => {
            let index = u32_to_usize(index);
            if index >= count {
                return Err(Error::invalid_argument(
                    "font collection index out of range",
                ));
            }
            offsets.skip(index * 4)?;
            directory_offset(&mut offsets, header_end)?
        }
        FontFace::PostScriptNames(names) => named(data, offsets, header_end, names, work)?,
    };
    Ok(Selected { offset, header_end })
}

/// Signatures are inert data here; checking their range is not authentication.
fn signature(r: &mut Cursor<'_>, length: usize) -> Result<()> {
    let at = r.position();
    let tag = r.u32()?;
    let size = u32_to_usize(r.u32()?);
    let start = u32_to_usize(r.u32()?);
    if tag == 0 && size == 0 && start == 0 {
        return Ok(());
    }
    if tag != SIGNATURE || size == 0 || start < here(r) {
        return Err(syntax(at, "invalid font collection signature range"));
    }
    let end = start
        .checked_add(size)
        .ok_or_else(|| limit(at, "collection signature size overflow"))?;
    if end > length {
        return Err(syntax(at, "font collection signature outside input"));
    }
    Ok(())
}

fn directory_offset(offsets: &mut Cursor<'_>, header_end: usize) -> Result<usize> {
    let at = offsets.position();
    let offset = u32_to_usize(offsets.u32()?);
    if offset < header_end {
        return Err(syntax(at, "font directory overlaps collection header"));
    }
    Ok(offset)
}

fn named(
    data: &[u8],
    mut offsets: Cursor<'_>,
    header_end: usize,
    names: &[&[u8]],
    work: &mut Work<'_>,
) -> Result<usize> {
    let at = offsets.position();
    work.work(names.len(), at)?;
    for name in names {
        work.string(name.len(), at)?;
    }
    let mut selected = None;
    let mut rank = usize::MAX;
    let mut ambiguous = false;
    while !offsets.at_end() {
        let offset = directory_offset(&mut offsets, header_end)?;
        let mut records = directory::records(data, offset, work)?;
        let mut table = None;
        while let Some(record) = records.next()? {
            if &record.tag == b"name" {
                record.check_overlap(&records.range, header_end)?;
                if table.replace(record.range).is_some() {
                    return Err(syntax(record.position, "duplicate collection name table"));
                }
            }
        }
        let Some(table) = table else { continue };
        let Some(candidate) = name_rank(
            slice(data, table, "collection name table outside font")?,
            names,
            work,
        )?
        else {
            continue;
        };
        if candidate < rank {
            selected = Some(offset);
            rank = candidate;
            ambiguous = false;
        } else if candidate == rank {
            ambiguous = true;
        }
    }
    if ambiguous {
        return Err(syntax(at, "ambiguous font collection name"));
    }
    selected.ok_or_else(|| syntax(at, "font collection name not found"))
}

fn name_rank(mut table: Cursor<'_>, names: &[&[u8]], work: &mut Work<'_>) -> Result<Option<usize>> {
    let base = here(&table);
    let version = table.u16()?;
    if version > 1 {
        // OpenType's version rule treats an unreadable table as absent.
        return Ok(None);
    }
    let count = usize::from(table.u16()?);
    let storage = usize::from(table.u16()?);
    work.work(count, table.position())?;
    let mut records = table.take_cursor(count * NAME_RECORD)?;
    let languages = if version == 1 {
        let count = usize::from(table.u16()?);
        table.skip(count * LANGUAGE_RECORD)?;
        count
    } else {
        0
    };
    if storage < table.offset() || storage > table.data().len() {
        return Err(syntax(
            Position::Buffer(base),
            "invalid name string storage",
        ));
    }
    let mut rank = None;
    while !records.at_end() {
        let platform = records.u16()?;
        let encoding = records.u16()?;
        let language = records.u16()?;
        let id = records.u16()?;
        let length = usize::from(records.u16()?);
        let offset = usize::from(records.u16()?);
        if id != POSTSCRIPT_NAME
            || (language >= LANGUAGE_TAG && usize::from(language - LANGUAGE_TAG) >= languages)
        {
            continue;
        }
        let Some(encoding) = Encoding::of(platform, encoding) else {
            continue;
        };
        let mut value = table.cursor_at(storage + offset)?.take_cursor(length)?;
        let at = value.position();
        work.string(length, at)?;
        work.work(length, at)?;
        let mut buffer = [0; MAX_NAME];
        let name = encoding.ascii(&mut value, &mut buffer)?;
        for (index, candidate) in names.iter().enumerate() {
            work.work(name.len(), at)?;
            if name == *candidate && rank.is_none_or(|rank| index < rank) {
                rank = Some(index);
            }
        }
    }
    Ok(rank)
}

enum Encoding {
    Ascii,
    Utf16,
}

impl Encoding {
    fn of(platform: u16, encoding: u16) -> Option<Self> {
        // OpenType name: Unicode and Windows use UTF-16BE, except Windows
        // code pages 936/950/949. Their ASCII subset and Macintosh Roman
        // suffice for name ID 6's restricted printable-ASCII alphabet.
        match (platform, encoding) {
            (0, _) | (3, 0..=2 | 6 | 10) => Some(Self::Utf16),
            (1, 0) | (3, 3..=5) => Some(Self::Ascii),
            _ => None,
        }
    }

    fn ascii<'a>(
        &self,
        value: &mut Cursor<'_>,
        buffer: &'a mut [u8; MAX_NAME],
    ) -> Result<&'a [u8]> {
        let at = value.position();
        let unit = match self {
            Self::Ascii => 1,
            Self::Utf16 => 2,
        };
        let count = value.remaining() / unit;
        if count == 0 || count > MAX_NAME || !value.remaining().is_multiple_of(unit) {
            return Err(syntax(at, "invalid PostScript font name length"));
        }
        for byte in buffer.iter_mut().take(count) {
            let character = match self {
                Self::Ascii => u16::from(value.u8()?),
                Self::Utf16 => value.u16()?,
            };
            let character = u8::try_from(character)
                .map_err(|_| syntax(at, "non-ASCII PostScript font name"))?;
            if !(b'!'..=b'~').contains(&character) || b"[](){}<>/%".contains(&character) {
                return Err(syntax(at, "invalid PostScript font name character"));
            }
            *byte = character;
        }
        Ok(&buffer[..count])
    }
}

#[cfg(test)]
mod tests;
