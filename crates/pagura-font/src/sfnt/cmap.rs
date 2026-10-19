//! Validated cmap formats 0, 2, 4, 6, 10, 12 and 13; selection never expands code ranges.
use crate::budget::Work;
use crate::read::{bad, cursor, here};
use crate::repairs;
use crate::{CmapSubtable, GlyphId};
use pagura_core::bytes::Cursor;
use pagura_core::convert::u32_to_usize;
use pagura_core::{Position, Result};
use std::ops::Range;

mod extended;

#[derive(Debug)]
pub(super) struct Cmap {
    pub selection: CmapSubtable,
    range: Range<usize>,
    format: u16,
}

pub(super) fn parse(mut r: Cursor<'_>, glyphs: usize, work: &mut Work<'_>) -> Result<Vec<Cmap>> {
    let start = here(&r);
    if r.u16()? != 0 {
        return Err(bad(here(&r), "invalid cmap version"));
    }
    let count = usize::from(r.u16()?);
    work.entries(count, Position::Buffer(here(&r)))?;
    work.work(count, Position::Buffer(here(&r)))?;
    let mut result = Vec::new();
    for _ in 0..count {
        if r.remaining() < 8 {
            work.repair(
                repairs::CMAP_TRUNCATED,
                "incomplete cmap directory",
                Position::Buffer(here(&r)),
            )?;
            break;
        }
        let platform = r.u16()?;
        let encoding = r.u16()?;
        let offset = u32_to_usize(r.u32()?);
        let mut sub = match r.cursor_at(offset) {
            Ok(sub) => sub,
            Err(error) => {
                work.repair(
                    repairs::CMAP_TRUNCATED,
                    error.message,
                    Position::Buffer(start.saturating_add(offset)),
                )?;
                continue;
            }
        };
        let format = match sub.u16() {
            Ok(format) => format,
            Err(error) => {
                work.repair(repairs::CMAP_TRUNCATED, error.message, error.position)?;
                continue;
            }
        };
        if !matches!(format, 0 | 2 | 4 | 6 | 10 | 12 | 13) {
            continue;
        }
        let length = match validate(r.cursor_at(offset)?, glyphs, work) {
            Ok(length) => length,
            Err(error) if malformed(&error) => {
                work.repair(repairs::CMAP_DATA, error.message, error.position)?;
                continue;
            }
            Err(error) => return Err(error),
        };
        if length == 0 {
            continue;
        }
        let selection = match (platform, encoding) {
            (3, 1) => CmapSubtable::WindowsUnicode,
            (3, 10) => CmapSubtable::WindowsUnicodeFull,
            (0, _) => CmapSubtable::Unicode,
            (3, 0) => CmapSubtable::WindowsSymbol,
            (1, 0) => CmapSubtable::MacRoman,
            _ => continue,
        };
        result.push(Cmap {
            selection,
            range: start + offset..start + offset + length,
            format,
        });
    }
    // Prefer full Unicode over BMP; keep the directory order for ties.
    result.sort_by_key(|c| {
        std::cmp::Reverse(match c.format {
            12 => 7,
            10 => 6,
            4 => 5,
            6 => 4,
            2 => 3,
            0 => 2,
            _ => 1,
        })
    });
    Ok(result)
}

fn validate(mut r: Cursor<'_>, glyphs: usize, work: &mut Work<'_>) -> Result<usize> {
    let start = here(&r);
    let format = r.u16()?;
    let length = if matches!(format, 10 | 12 | 13) {
        if r.u16()? != 0 {
            return Err(bad(here(&r), "invalid cmap reserved field"));
        }
        u32_to_usize(r.u32()?)
    } else {
        usize::from(r.u16()?)
    };
    let length = if length > r.data().len() {
        work.repair(
            repairs::CMAP_TRUNCATED,
            "cmap length exceeds available data",
            Position::Buffer(start),
        )?;
        r.data().len()
    } else {
        length
    };
    let mut r = cursor(r.data().get(..length).unwrap_or_default()).with_base(start);
    let header = match format {
        0 => 6,
        2 => 518,
        4 | 12 | 13 => 16,
        6 => 10,
        _ => 20,
    };
    if length < header {
        work.repair(
            repairs::CMAP_TRUNCATED,
            "incomplete cmap subtable header",
            Position::Buffer(start),
        )?;
        return Ok(0);
    }
    match format {
        0 => {
            r.skip(6)?;
            work.work(256, Position::Buffer(here(&r)))?;
            let count = available(&r, 256, 1, work)?;
            for &glyph in r.take(count)? {
                if usize::from(glyph) >= glyphs {
                    return Err(bad(here(&r), "invalid cmap glyph"));
                }
            }
        }
        2 => extended::validate_two(r, glyphs, work)?,
        10 => extended::validate_ten(r, glyphs, work)?,
        6 => {
            r.skip(6)?;
            let first = usize::from(r.u16()?);
            let count = usize::from(r.u16()?);
            if first + count > 65536 {
                return Err(bad(here(&r), "invalid trimmed cmap range"));
            }
            work.work(count, Position::Buffer(here(&r)))?;
            let count = available(&r, count, 2, work)?;
            for _ in 0..count {
                if usize::from(r.u16()?) >= glyphs {
                    return Err(bad(here(&r), "invalid cmap glyph"));
                }
            }
        }
        4 => validate_four(r, glyphs, work)?,
        12 | 13 => {
            r.skip(12)?;
            let count = u32_to_usize(r.u32()?);
            work.entries(count, Position::Buffer(here(&r)))?;
            work.work(count, Position::Buffer(here(&r)))?;
            let count = available(&r, count, 12, work)?;
            let mut previous = None;
            for _ in 0..count {
                let start = r.u32()?;
                let end = r.u32()?;
                let gid = r.u32()?;
                if start > end
                    || end > 0x0010_ffff
                    || previous.is_some_and(|p| start <= p)
                    || gid
                        .checked_add(if format == 13 { 0 } else { end - start })
                        .is_none_or(|g| u64::from(g) >= glyphs as u64)
                {
                    return Err(bad(here(&r), "invalid cmap group"));
                }
                previous = Some(end);
            }
        }
        _ => return Err(bad(here(&r), "invalid cmap format")),
    }
    Ok(length)
}

fn validate_four(mut r: Cursor<'_>, glyphs: usize, work: &mut Work<'_>) -> Result<()> {
    let table = here(&r);
    r.skip(6)?;
    let twice = usize::from(r.u16()?);
    if twice == 0 || twice % 2 != 0 {
        return Err(bad(here(&r), "invalid cmap segment count"));
    }
    let count = twice / 2;
    work.entries(count, Position::Buffer(here(&r)))?;
    r.skip(6)?;
    let ends = r.take(twice)?;
    if r.u16()? != 0 {
        return Err(bad(here(&r), "invalid cmap padding"));
    }
    let starts = r.take(twice)?;
    r.skip(twice)?;
    let range_pos = r.offset();
    r.take(twice)?;
    // Ordered end codes let validation use the segment it is already walking,
    // rather than binary-searching it again for every character. Keep the old
    // lookup on disordered input, including its error/repair discovery order.
    let ordered = ends.as_chunks::<2>().0.windows(2).all(|w| w[0] < w[1]);
    let mut previous = None;
    for i in 0..count {
        let end = u16::from_be_bytes([ends[2 * i], ends[2 * i + 1]]);
        let start = u16::from_be_bytes([starts[2 * i], starts[2 * i + 1]]);
        if start > end || previous.is_some_and(|p| start <= p) {
            return Err(bad(here(&r), "overlapping cmap segments"));
        }
        previous = Some(end);
        work.work(usize::from(end - start) + 1, Position::Buffer(here(&r)))?;
        let offset = word(r.data(), range_pos + 2 * i)?;
        if offset % 2 != 0 {
            work.repair(
                repairs::CMAP_ALIGNMENT,
                "unaligned cmap glyph array",
                Position::Buffer(table + range_pos + 2 * i),
            )?;
        }
        for code in start..=end {
            let glyph = match if ordered {
                lookup_four_segment(r.data(), code, count, range_pos, i)
            } else {
                lookup_four(r.data(), code, count, range_pos)
            } {
                Ok(glyph) => glyph,
                Err(error) if malformed(&error) => {
                    work.repair(
                        repairs::CMAP_TRUNCATED,
                        error.message,
                        Position::Buffer(table + range_pos + 2 * i),
                    )?;
                    continue;
                }
                Err(error) => return Err(error),
            };
            if usize::from(glyph) >= glyphs {
                return Err(bad(here(&r), "invalid cmap glyph"));
            }
        }
    }
    if previous != Some(65535) {
        return Err(bad(here(&r), "missing cmap sentinel"));
    }
    Ok(())
}

fn word(data: &[u8], pos: usize) -> Result<u16> {
    let mut r = cursor(data);
    r.skip(pos)?;
    r.u16()
}
fn lookup_four(data: &[u8], code: u16, count: usize, ranges: usize) -> Result<u16> {
    // Binary search the endCode array; all array extents were validated.
    let mut low = 0;
    let mut high = count;
    while low < high {
        let mid = low + (high - low) / 2;
        if word(data, 14 + 2 * mid)? < code {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    if low == count {
        return Ok(0);
    }
    lookup_four_segment(data, code, count, ranges, low)
}

fn lookup_four_segment(
    data: &[u8],
    code: u16,
    count: usize,
    ranges: usize,
    low: usize,
) -> Result<u16> {
    let start = word(data, 16 + 2 * count + 2 * low)?;
    if code < start {
        return Ok(0);
    }
    let delta = word(data, 16 + 4 * count + 2 * low)?;
    let offset = word(data, ranges + 2 * low)?;
    if offset == 0 {
        return Ok(code.wrapping_add(delta));
    }
    // Some fonts use ffff in the mandatory missing-code sentinel segment.
    // Validation admits its alignment repair before this lookup is exposed.
    if code == 65535 && start == 65535 && offset == 65535 {
        return Ok(0);
    }
    let glyph = word(
        data,
        ranges + 2 * low + usize::from(offset) + 2 * usize::from(code - start),
    )?;
    Ok(if glyph == 0 {
        0
    } else {
        glyph.wrapping_add(delta)
    })
}

impl Cmap {
    pub(super) fn lookup(&self, data: &[u8], code: u32) -> Option<GlyphId> {
        let mut r = cursor(data.get(self.range.clone())?).with_base(self.range.start);
        let glyph = match self.format {
            0 => {
                if code > 255 {
                    return None;
                }
                r.skip(6 + usize::try_from(code).ok()?).ok()?;
                u32::from(r.u8().ok()?)
            }
            2 => {
                if code > 65535 {
                    return None;
                }
                u32::from(extended::lookup_two(r.data(), code).ok()?)
            }
            6 | 10 => {
                let (first, count) = if self.format == 10 {
                    r.skip(12).ok()?;
                    (r.u32().ok()?, r.u32().ok()?)
                } else {
                    r.skip(6).ok()?;
                    (u32::from(r.u16().ok()?), u32::from(r.u16().ok()?))
                };
                if code < first || code - first >= count {
                    return None;
                }
                r.skip(usize::try_from(code - first).ok()? * 2).ok()?;
                u32::from(r.u16().ok()?)
            }
            4 => {
                let code = u16::try_from(code).ok()?;
                r.skip(6).ok()?;
                let count = usize::from(r.u16().ok()?) / 2;
                u32::from(lookup_four(r.data(), code, count, 16 + 6 * count).ok()?)
            }
            12 | 13 => {
                r.skip(12).ok()?;
                let count = u32_to_usize(r.u32().ok()?).min(r.data().len().saturating_sub(16) / 12);
                let mut low = 0;
                let mut high = count;
                while low < high {
                    let mid = low + (high - low) / 2;
                    let mut group = r.cursor_at(16 + 12 * mid).ok()?;
                    let start = group.u32().ok()?;
                    let end = group.u32().ok()?;
                    let gid = group.u32().ok()?;
                    if code < start {
                        high = mid;
                    } else if code > end {
                        low = mid + 1;
                    } else {
                        return Some(GlyphId(
                            gid + if self.format == 13 { 0 } else { code - start },
                        ));
                    }
                }
                return None;
            }
            _ => return None,
        };
        Some(GlyphId(glyph))
    }
}

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
            Position::Buffer(here(r)),
        )?;
    }
    Ok(count.min(available))
}

fn malformed(error: &pagura_core::Error) -> bool {
    error.repair.is_none() && error.kind == pagura_core::ErrorKind::Syntax
}

#[cfg(test)]
mod tests {
    use super::{lookup_four, validate_four};
    use crate::budget::{FontBudget, Work};
    use crate::read::cursor;
    use pagura_core::{Diagnostics, ErrorKind, Limits, Position, Recovery};

    fn format_four() -> Vec<u8> {
        // Three segments: 20..22 use delta -31; 30..32 use a glyph array
        // and delta -1; ffff is the required missing-code sentinel.
        [
            4_u16, 46, 0, 6, 4, 1, 2, 0x22, 0x32, 0xffff, 0, 0x20, 0x30, 0xffff, 0xffe1, 0xffff, 1,
            0, 4, 0, 5, 0, 9,
        ]
        .into_iter()
        .flat_map(u16::to_be_bytes)
        .collect()
    }

    #[test]
    fn format_four_validates_both_mapping_routes_and_keeps_work_charges() {
        let data = format_four();
        let mut diagnostics = Diagnostics::new(Recovery::Strict);
        // Seven codes are validated: one work unit each.
        for (work, valid) in [(7, true), (6, false)] {
            let limits = Limits {
                font_work: work,
                ..Limits::default()
            };
            let mut work = Work::new(FontBudget::new(&limits), &mut diagnostics);
            let result = validate_four(cursor(&data), 9, &mut work);
            if valid {
                result.unwrap();
            } else {
                assert_eq!(result.unwrap_err().kind, ErrorKind::Limit);
            }
        }
        assert_eq!(diagnostics.as_slice(), []);
        for (code, glyph) in [
            (0x1f, 0),
            (0x20, 1),
            (0x21, 2),
            (0x22, 3),
            (0x23, 0),
            (0x30, 4),
            (0x31, 0),
            (0x32, 8),
            (0xffff, 0),
        ] {
            assert_eq!(lookup_four(&data, code, 3, 34).unwrap(), glyph);
        }
    }

    #[test]
    fn disordered_end_codes_preserve_the_structural_error_position() {
        let mut data = format_four();
        data[14..16].copy_from_slice(&0x32_u16.to_be_bytes());
        data[16..18].copy_from_slice(&0x22_u16.to_be_bytes());
        let mut diagnostics = Diagnostics::new(Recovery::Strict);
        let mut work = Work::new(FontBudget::new(&Limits::default()), &mut diagnostics);
        let error = validate_four(cursor(&data), 9, &mut work).unwrap_err();
        assert_eq!(error.message, "overlapping cmap segments");
        assert_eq!(error.position, Position::Buffer(40));
    }
}
