//! Validated cmap formats 0, 2, 4, 6, 8, 10, 12 and 13; selection never expands code ranges.
//!
//! Subtables are validated once, while parsing. Lookups then read the
//! validated bytes with bounds-checked accessors: a mapping that a repaired,
//! truncated subtable no longer holds is simply absent.
use crate::budget::Work;
use crate::read::here;
use crate::{CmapSubtable, GlyphId, repairs};
use pagura_core::bytes::Cursor;
use pagura_core::convert::u32_to_usize;
use pagura_core::{Error, ErrorKind, Position, Result};
use std::ops::Range;

mod extended;

/// cmap encoding-record platforms and encodings (OpenType cmap).
const PLATFORM_UNICODE: u16 = 0;
const PLATFORM_MACINTOSH: u16 = 1;
const PLATFORM_WINDOWS: u16 = 3;
const MAC_ROMAN: u16 = 0;
const WINDOWS_SYMBOL: u16 = 0;
const WINDOWS_BMP: u16 = 1;
const WINDOWS_FULL: u16 = 10;
/// The bytes of one encoding record.
const ENCODING_RECORD: usize = 8;
/// The largest Unicode scalar value.
const MAX_UNICODE: u32 = 0x0010_ffff;
/// The 16-bit code space of formats 2, 4 and 6.
const SHORT_CODES: usize = 0x1_0000;
/// The format 4 sentinel segment ends at this code.
const SENTINEL: u16 = 0xffff;

/// The subtable formats this reader validates and looks up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    /// 0: byte encoding table.
    Bytes,
    /// 2: high-byte mapping through subheaders.
    HighByte,
    /// 4: segment mapping to delta values.
    Segments,
    /// 6: trimmed table mapping.
    Trimmed,
    /// 8: mixed 16/32-bit codes, already delimited by the caller.
    MixedLength,
    /// 10: trimmed array (32-bit codes).
    TrimmedArray,
    /// 12: segmented coverage.
    Coverage,
    /// 13: many-to-one range mappings.
    ManyToOne,
}

impl Format {
    const fn new(code: u16) -> Option<Self> {
        Some(match code {
            0 => Self::Bytes,
            2 => Self::HighByte,
            4 => Self::Segments,
            6 => Self::Trimmed,
            8 => Self::MixedLength,
            10 => Self::TrimmedArray,
            12 => Self::Coverage,
            13 => Self::ManyToOne,
            _ => return None,
        })
    }

    /// Whether the header has a reserved word and a 32-bit length.
    const fn long_header(self) -> bool {
        matches!(
            self,
            Self::MixedLength | Self::TrimmedArray | Self::Coverage | Self::ManyToOne
        )
    }

    /// The length of the fixed header, before the format's arrays.
    const fn header(self) -> usize {
        match self {
            Self::Bytes => 6,
            Self::HighByte => extended::SUBHEADERS,
            Self::Segments | Self::Coverage | Self::ManyToOne => 16,
            Self::Trimmed => 10,
            // Twelve header bytes, the 8192-byte is32 bitmap, then numGroups.
            Self::MixedLength => 8208,
            Self::TrimmedArray => 20,
        }
    }

    /// Selection preference: full Unicode before BMP; ties keep the
    /// directory order.
    const fn preference(self) -> u8 {
        match self {
            Self::Coverage => 8,
            Self::TrimmedArray => 7,
            Self::MixedLength => 6,
            Self::Segments => 5,
            Self::Trimmed => 4,
            Self::HighByte => 3,
            Self::Bytes => 2,
            Self::ManyToOne => 1,
        }
    }
}

/// What a lookup needs beyond the subtable bytes.
#[derive(Debug, Clone, Copy)]
enum Layout {
    Bytes,
    HighByte,
    /// Format 4 with `count` segments.
    Segments {
        count: usize,
    },
    /// Formats 6 and 10: `count` glyphs for codes from `first`, at `array`.
    Trimmed {
        first: u32,
        count: u32,
        array: usize,
    },
    /// Formats 8, 12 and 13 with `count` complete groups.
    Groups {
        count: usize,
        format: Format,
    },
}

#[derive(Debug)]
pub(super) struct Cmap {
    selection: CmapSubtable,
    range: Range<usize>,
    layout: Layout,
}

pub(super) fn parse(mut r: Cursor<'_>, glyphs: usize, work: &mut Work<'_>) -> Result<Vec<Cmap>> {
    let table = here(&r);
    if r.u16()? != 0 {
        return Err(r.error("invalid cmap version"));
    }
    let count = usize::from(r.u16()?);
    work.entries(count, r.position())?;
    work.work(count, r.position())?;
    let mut result = Vec::new();
    for _ in 0..count {
        if r.remaining() < ENCODING_RECORD {
            work.repair(
                repairs::CMAP_TRUNCATED,
                "incomplete cmap directory",
                r.position(),
            )?;
            break;
        }
        let platform = r.u16()?;
        let encoding = r.u16()?;
        let offset = u32_to_usize(r.u32()?);
        let format = match r.cursor_at(offset).and_then(|mut sub| sub.u16()) {
            Ok(format) => format,
            Err(error) => {
                work.repair(repairs::CMAP_TRUNCATED, error.message, error.position)?;
                continue;
            }
        };
        let Some(format) = Format::new(format) else {
            continue;
        };
        let (length, layout) = match validate(r.cursor_at(offset)?, format, glyphs, work) {
            Ok(Some(valid)) => valid,
            Ok(None) => continue,
            Err(error) if malformed(&error) => {
                work.repair(repairs::CMAP_DATA, error.message, error.position)?;
                continue;
            }
            Err(error) => return Err(error),
        };
        let selection = match (platform, encoding) {
            (PLATFORM_WINDOWS, WINDOWS_BMP) => CmapSubtable::WindowsUnicode,
            (PLATFORM_WINDOWS, WINDOWS_FULL) => CmapSubtable::WindowsUnicodeFull,
            (PLATFORM_UNICODE, _) => CmapSubtable::Unicode,
            (PLATFORM_WINDOWS, WINDOWS_SYMBOL) => CmapSubtable::WindowsSymbol,
            (PLATFORM_MACINTOSH, MAC_ROMAN) => CmapSubtable::MacRoman,
            _ => continue,
        };
        let start = table + offset;
        result.push((
            format,
            Cmap {
                selection,
                range: start..start + length,
                layout,
            },
        ));
    }
    result.sort_by_key(|(format, _)| std::cmp::Reverse(format.preference()));
    Ok(result.into_iter().map(|(_, cmap)| cmap).collect())
}

/// Validates one subtable and returns its usable length and layout, or
/// `None` if its header is incomplete (a reported repair).
fn validate(
    mut r: Cursor<'_>,
    format: Format,
    glyphs: usize,
    work: &mut Work<'_>,
) -> Result<Option<(usize, Layout)>> {
    let start = r.position();
    r.u16()?;
    let length = if format.long_header() {
        if r.u16()? != 0 {
            return Err(r.error("invalid cmap reserved field"));
        }
        u32_to_usize(r.u32()?)
    } else {
        usize::from(r.u16()?)
    };
    let length = if length > r.data().len() {
        work.repair(
            repairs::CMAP_TRUNCATED,
            "cmap length exceeds available data",
            start,
        )?;
        r.data().len()
    } else {
        length
    };
    if length < format.header() {
        work.repair(
            repairs::CMAP_TRUNCATED,
            "incomplete cmap subtable header",
            start,
        )?;
        return Ok(None);
    }
    let mut r = r.cursor_at(0)?.take_cursor(length)?;
    let layout = match format {
        Format::Bytes => {
            r.skip(6)?;
            work.work(256, r.position())?;
            let count = available(&r, 256, 1, work)?;
            for &glyph in r.take(count)? {
                if usize::from(glyph) >= glyphs {
                    return Err(r.error("invalid cmap glyph"));
                }
            }
            Layout::Bytes
        }
        Format::HighByte => {
            extended::validate_two(r, glyphs, work)?;
            Layout::HighByte
        }
        Format::TrimmedArray => extended::validate_ten(r, glyphs, work)?,
        Format::Trimmed => {
            r.skip(6)?;
            let first = r.u16()?;
            let count = r.u16()?;
            if usize::from(first) + usize::from(count) > SHORT_CODES {
                return Err(r.error("invalid trimmed cmap range"));
            }
            work.work(usize::from(count), r.position())?;
            for _ in 0..available(&r, usize::from(count), 2, work)? {
                if usize::from(r.u16()?) >= glyphs {
                    return Err(r.error("invalid cmap glyph"));
                }
            }
            Layout::Trimmed {
                first: u32::from(first),
                count: u32::from(count),
                array: Format::Trimmed.header(),
            }
        }
        Format::Segments => Layout::Segments {
            count: validate_four(r, glyphs, work)?,
        },
        Format::MixedLength | Format::Coverage | Format::ManyToOne => {
            validate_groups(r, format, glyphs, work)?
        }
    };
    Ok(Some((length, layout)))
}

fn validate_groups(
    mut r: Cursor<'_>,
    format: Format,
    glyphs: usize,
    work: &mut Work<'_>,
) -> Result<Layout> {
    let array = format.header();
    // Format 8's is32 bitmap delimits words in an encoded text stream. This
    // API receives a complete numeric code, so lookup needs no word decoder
    // or UTF-16 conversion. Both its bitmap and group count must be present.
    r.skip(array - 4)?;
    let count = u32_to_usize(r.u32()?);
    work.entries(count, r.position())?;
    work.work(count, r.position())?;
    let count = available(&r, count, 12, work)?;
    let many_to_one = format == Format::ManyToOne;
    let mut previous = None;
    for _ in 0..count {
        let start = r.u32()?;
        let end = r.u32()?;
        let gid = r.u32()?;
        if start > end
            || (format != Format::MixedLength && end > MAX_UNICODE)
            || previous.is_some_and(|p| start <= p)
            || gid
                .checked_add(if many_to_one { 0 } else { end - start })
                .is_none_or(|g| u32_to_usize(g) >= glyphs)
        {
            return Err(r.error("invalid cmap group"));
        }
        previous = Some(end);
    }
    Ok(Layout::Groups { count, format })
}

/// The arrays of a format 4 subtable: segment ends, starts, deltas and
/// range offsets, each `2 × count` bytes after a 14-byte header and a pad.
struct Segments<'a> {
    data: &'a [u8],
    count: usize,
}

impl Segments<'_> {
    fn end(&self, i: usize) -> Option<u16> {
        u16_at(self.data, 14 + 2 * i)
    }

    fn start(&self, i: usize) -> Option<u16> {
        u16_at(self.data, 16 + 2 * self.count + 2 * i)
    }

    fn delta(&self, i: usize) -> Option<u16> {
        u16_at(self.data, 16 + 4 * self.count + 2 * i)
    }

    /// The offset of segment `i`'s idRangeOffset word.
    fn range_offset_at(&self, i: usize) -> usize {
        16 + 6 * self.count + 2 * i
    }

    /// The glyph of `code` in segment `i`, which covers it; `None` if the
    /// glyph array entry is missing.
    fn glyph(&self, i: usize, code: u16) -> Option<u16> {
        let start = self.start(i)?;
        let delta = self.delta(i)?;
        let at = self.range_offset_at(i);
        let offset = u16_at(self.data, at)?;
        if offset == 0 {
            return Some(code.wrapping_add(delta));
        }
        // Some fonts use ffff in the mandatory missing-code sentinel segment.
        // Validation admits its alignment repair before this lookup is exposed.
        if code == SENTINEL && start == SENTINEL && offset == SENTINEL {
            return Some(0);
        }
        let glyph = u16_at(
            self.data,
            at + usize::from(offset) + 2 * usize::from(code - start),
        )?;
        Some(if glyph == 0 {
            0
        } else {
            glyph.wrapping_add(delta)
        })
    }

    /// The glyph of `code`: binary search for the first segment ending at
    /// or after it.
    fn lookup(&self, code: u16) -> Option<u16> {
        let (mut low, mut high) = (0, self.count);
        while low < high {
            let mid = low + (high - low) / 2;
            if self.end(mid)? < code {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        if low == self.count || code < self.start(low)? {
            return Some(0);
        }
        self.glyph(low, code)
    }
}

/// Validates a format 4 subtable and returns its segment count.
fn validate_four(mut r: Cursor<'_>, glyphs: usize, work: &mut Work<'_>) -> Result<usize> {
    let base = here(&r);
    r.skip(6)?;
    let twice = usize::from(r.u16()?);
    if twice == 0 || !twice.is_multiple_of(2) {
        return Err(r.error("invalid cmap segment count"));
    }
    let count = twice / 2;
    work.entries(count, r.position())?;
    r.skip(6)?; // searchRange, entrySelector, rangeShift
    r.skip(twice)?; // endCode
    if r.u16()? != 0 {
        return Err(r.error("invalid cmap padding"));
    }
    r.skip(twice)?; // startCode
    r.skip(twice)?; // idDelta
    r.skip(twice)?; // idRangeOffset
    let segments = Segments {
        data: r.data(),
        count,
    };
    let invalid = || r.error("overlapping cmap segments");
    // Ordered end codes let validation use the segment it is already walking,
    // rather than binary-searching it again for every character. Keep the
    // search on disordered input, including its error/repair discovery order.
    let ordered = (1..count)
        .all(|i| matches!((segments.end(i - 1), segments.end(i)), (Some(a), Some(b)) if a < b));
    let mut previous = None;
    for i in 0..count {
        let (Some(end), Some(start)) = (segments.end(i), segments.start(i)) else {
            return Err(invalid());
        };
        if start > end || previous.is_some_and(|p| start <= p) {
            return Err(invalid());
        }
        previous = Some(end);
        work.work(usize::from(end - start) + 1, r.position())?;
        let range_at = Position::Buffer(base + segments.range_offset_at(i));
        if u16_at(segments.data, segments.range_offset_at(i)).is_some_and(|offset| offset % 2 != 0)
        {
            work.repair(
                repairs::CMAP_ALIGNMENT,
                "unaligned cmap glyph array",
                range_at,
            )?;
        }
        for code in start..=end {
            let glyph = if ordered {
                segments.glyph(i, code)
            } else {
                segments.lookup(code)
            };
            let Some(glyph) = glyph else {
                work.repair(
                    repairs::CMAP_TRUNCATED,
                    "truncated cmap glyph array",
                    range_at,
                )?;
                continue;
            };
            if usize::from(glyph) >= glyphs {
                return Err(r.error("invalid cmap glyph"));
            }
        }
    }
    if previous != Some(SENTINEL) {
        return Err(r.error("missing cmap sentinel"));
    }
    Ok(count)
}

impl Cmap {
    pub(super) const fn selection(&self) -> CmapSubtable {
        self.selection
    }

    pub(super) fn lookup(&self, data: &[u8], code: u32) -> Option<GlyphId> {
        let bytes = data.get(self.range.clone())?;
        let glyph = match self.layout {
            Layout::Bytes => u32::from(*bytes.get(6 + usize::from(u8::try_from(code).ok()?))?),
            Layout::HighByte => u32::from(extended::lookup_two(bytes, u16::try_from(code).ok()?)?),
            Layout::Trimmed {
                first,
                count,
                array,
            } => {
                let index = code.checked_sub(first).filter(|&i| i < count)?;
                u32::from(u16_at(bytes, array + 2 * u32_to_usize(index))?)
            }
            Layout::Segments { count } => {
                let segments = Segments { data: bytes, count };
                u32::from(segments.lookup(u16::try_from(code).ok()?)?)
            }
            Layout::Groups { count, format } => {
                let array = format.header();
                let many_to_one = format == Format::ManyToOne;
                let (mut low, mut high) = (0, count);
                loop {
                    if low >= high {
                        return None;
                    }
                    let mid = low + (high - low) / 2;
                    let at = array + 12 * mid;
                    let (start, end, gid) = (
                        u32_at(bytes, at)?,
                        u32_at(bytes, at + 4)?,
                        u32_at(bytes, at + 8)?,
                    );
                    if code < start {
                        high = mid;
                    } else if code > end {
                        low = mid + 1;
                    } else {
                        break gid + if many_to_one { 0 } else { code - start };
                    }
                }
            }
        };
        Some(GlyphId(glyph))
    }
}

pub(super) fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(*data.get(at..)?.first_chunk()?))
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(*data.get(at..)?.first_chunk()?))
}

/// At most `count` records of `stride` bytes remain; fewer is a reported
/// truncation.
pub(super) fn available(
    r: &Cursor<'_>,
    count: usize,
    stride: usize,
    work: &mut Work<'_>,
) -> Result<usize> {
    let available = r.remaining() / stride;
    if count > available {
        work.repair(
            repairs::CMAP_TRUNCATED,
            "cmap entries exceed available data",
            r.position(),
        )?;
    }
    Ok(count.min(available))
}

fn malformed(error: &Error) -> bool {
    error.repair.is_none() && error.kind == ErrorKind::Syntax
}

#[cfg(test)]
mod tests;
