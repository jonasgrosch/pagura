//! Validated cmap formats 0, 2, 4, 6, 10, 12 and 13; selection never expands code ranges.
use crate::read::{Read, bad, cap};
use crate::{CmapSubtable, GlyphId};
use crate::{recovery::repair, tags};
use pagura_core::{Diagnostics, Limits, Position, Result};
use std::ops::Range;

mod extended;

#[derive(Debug)]
pub(super) struct Cmap {
    pub selection: CmapSubtable,
    range: Range<usize>,
    format: u16,
}

pub(super) fn parse(
    mut r: Read<'_>,
    glyphs: usize,
    limits: &Limits,
    diagnostics: &mut Diagnostics,
) -> Result<Vec<Cmap>> {
    if r.u16()? != 0 {
        return Err(bad(r.at(), "invalid cmap version"));
    }
    let count = usize::from(r.u16()?);
    cap(count, limits.objects.min(limits.value_nodes), r.at())?;
    let mut work = count;
    let mut result = Vec::new();
    for _ in 0..count {
        if r.data.len() - r.pos < 8 {
            repair(
                diagnostics,
                tags::CMAP_TRUNCATED,
                "incomplete cmap directory",
                Position::Buffer(r.at()),
            )?;
            break;
        }
        let platform = r.u16()?;
        let encoding = r.u16()?;
        let offset = r.offset()?;
        let mut sub = match r.tail(offset) {
            Ok(sub) => sub,
            Err(error) => {
                repair(
                    diagnostics,
                    tags::CMAP_TRUNCATED,
                    error.message,
                    Position::Buffer(r.base.saturating_add(offset)),
                )?;
                continue;
            }
        };
        let format = match sub.u16() {
            Ok(format) => format,
            Err(error) => {
                repair(
                    diagnostics,
                    tags::CMAP_TRUNCATED,
                    error.message,
                    error.position,
                )?;
                continue;
            }
        };
        if !matches!(format, 0 | 2 | 4 | 6 | 10 | 12 | 13) {
            continue;
        }
        let length = match validate(r.tail(offset)?, glyphs, limits, &mut work, diagnostics) {
            Ok(length) => length,
            Err(error) if malformed(&error) => {
                repair(diagnostics, tags::CMAP_DATA, error.message, error.position)?;
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
            range: r.base + offset..r.base + offset + length,
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

fn validate(
    mut r: Read<'_>,
    glyphs: usize,
    limits: &Limits,
    work: &mut usize,
    diagnostics: &mut Diagnostics,
) -> Result<usize> {
    let format = r.u16()?;
    let length = if matches!(format, 10 | 12 | 13) {
        if r.u16()? != 0 {
            return Err(bad(r.at(), "invalid cmap reserved field"));
        }
        r.offset()?
    } else {
        usize::from(r.u16()?)
    };
    let length = if length > r.data.len() {
        repair(
            diagnostics,
            tags::CMAP_TRUNCATED,
            "cmap length exceeds available data",
            Position::Buffer(r.base),
        )?;
        r.data.len()
    } else {
        length
    };
    let mut r = Read::new(&r.data[..length], r.base);
    let header = match format {
        0 => 6,
        2 => 518,
        4 | 12 | 13 => 16,
        6 => 10,
        _ => 20,
    };
    if length < header {
        repair(
            diagnostics,
            tags::CMAP_TRUNCATED,
            "incomplete cmap subtable header",
            Position::Buffer(r.base),
        )?;
        return Ok(0);
    }
    match format {
        0 => {
            r.skip(6)?;
            *work = work.saturating_add(256);
            cap(*work, limits.operations, r.at())?;
            let count = available(&r, 256, 1, diagnostics)?;
            for &glyph in r.take(count)? {
                if usize::from(glyph) >= glyphs {
                    return Err(bad(r.at(), "invalid cmap glyph"));
                }
            }
        }
        2 => extended::validate_two(r, glyphs, limits, work, diagnostics)?,
        10 => extended::validate_ten(r, glyphs, limits, work, diagnostics)?,
        6 => {
            r.skip(6)?;
            let first = usize::from(r.u16()?);
            let count = usize::from(r.u16()?);
            if first + count > 65536 {
                return Err(bad(r.at(), "invalid trimmed cmap range"));
            }
            *work = work.saturating_add(count);
            cap(*work, limits.operations, r.at())?;
            let count = available(&r, count, 2, diagnostics)?;
            for _ in 0..count {
                if usize::from(r.u16()?) >= glyphs {
                    return Err(bad(r.at(), "invalid cmap glyph"));
                }
            }
        }
        4 => validate_four(r, glyphs, limits, work, diagnostics)?,
        12 | 13 => {
            r.skip(12)?;
            let count = r.offset()?;
            *work = work.saturating_add(count);
            cap(count, limits.value_nodes, r.at())?;
            cap(*work, limits.operations, r.at())?;
            let count = available(&r, count, 12, diagnostics)?;
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
                    return Err(bad(r.at(), "invalid cmap group"));
                }
                previous = Some(end);
            }
        }
        _ => return Err(bad(r.at(), "invalid cmap format")),
    }
    Ok(length)
}

fn validate_four(
    mut r: Read<'_>,
    glyphs: usize,
    limits: &Limits,
    work: &mut usize,
    diagnostics: &mut Diagnostics,
) -> Result<()> {
    r.skip(6)?;
    let twice = usize::from(r.u16()?);
    if twice == 0 || twice % 2 != 0 {
        return Err(bad(r.at(), "invalid cmap segment count"));
    }
    let count = twice / 2;
    cap(count, limits.value_nodes, r.at())?;
    r.skip(6)?;
    let ends = r.take(twice)?;
    if r.u16()? != 0 {
        return Err(bad(r.at(), "invalid cmap padding"));
    }
    let starts = r.take(twice)?;
    r.skip(twice)?;
    let range_pos = r.pos;
    r.take(twice)?;
    let mut previous = None;
    for i in 0..count {
        let end = u16::from_be_bytes([ends[2 * i], ends[2 * i + 1]]);
        let start = u16::from_be_bytes([starts[2 * i], starts[2 * i + 1]]);
        if start > end || previous.is_some_and(|p| start <= p) {
            return Err(bad(r.at(), "overlapping cmap segments"));
        }
        previous = Some(end);
        *work = work.saturating_add(usize::from(end - start) + 1);
        cap(*work, limits.operations, r.at())?;
        let offset = word(r.data, range_pos + 2 * i)?;
        if offset % 2 != 0 {
            repair(
                diagnostics,
                tags::CMAP_ALIGNMENT,
                "unaligned cmap glyph array",
                Position::Buffer(r.base + range_pos + 2 * i),
            )?;
        }
        for code in start..=end {
            let glyph = match lookup_four(r.data, code, count, range_pos) {
                Ok(glyph) => glyph,
                Err(error) if malformed(&error) => {
                    repair(
                        diagnostics,
                        tags::CMAP_TRUNCATED,
                        error.message,
                        Position::Buffer(r.base + range_pos + 2 * i),
                    )?;
                    continue;
                }
                Err(error) => return Err(error),
            };
            if usize::from(glyph) >= glyphs {
                return Err(bad(r.at(), "invalid cmap glyph"));
            }
        }
    }
    if previous != Some(65535) {
        return Err(bad(r.at(), "missing cmap sentinel"));
    }
    Ok(())
}

fn word(data: &[u8], pos: usize) -> Result<u16> {
    let mut r = Read::new(data, 0);
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
    pub fn lookup(&self, data: &[u8], code: u32) -> Option<GlyphId> {
        let mut r = Read::new(data.get(self.range.clone())?, self.range.start);
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
                u32::from(extended::lookup_two(r.data, code).ok()?)
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
                u32::from(lookup_four(r.data, code, count, 16 + 6 * count).ok()?)
            }
            12 | 13 => {
                r.skip(12).ok()?;
                let count = r.offset().ok()?.min(r.data.len().saturating_sub(16) / 12);
                let mut low = 0;
                let mut high = count;
                while low < high {
                    let mid = low + (high - low) / 2;
                    let mut group = r.tail(16 + 12 * mid).ok()?;
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

fn charge(work: &mut usize, count: usize, limits: &Limits, at: usize) -> Result<()> {
    *work = work.saturating_add(count);
    cap(*work, limits.operations, at)
}

pub(super) fn available(
    r: &Read<'_>,
    count: usize,
    stride: usize,
    diagnostics: &mut Diagnostics,
) -> Result<usize> {
    let available = (r.data.len() - r.pos) / stride;
    if count > available {
        repair(
            diagnostics,
            tags::CMAP_TRUNCATED,
            "cmap entries exceed available data",
            Position::Buffer(r.at()),
        )?;
    }
    Ok(count.min(available))
}

fn malformed(error: &pagura_core::Error) -> bool {
    error.repair.is_none() && error.kind == pagura_core::ErrorKind::Syntax
}
