//! Mixed-byte and trimmed Unicode cmap layouts, pending OpenType primary review.
use super::{Layout, available, u16_at};
use crate::budget::Work;
use crate::repairs;
use pagura_core::Result;
use pagura_core::bytes::Cursor;
use pagura_core::convert::u32_to_usize;

/// Format 2: 256 subheader keys follow the 6-byte header, then the
/// subheaders of 8 bytes each (OpenType cmap format 2).
const KEYS: usize = 6;
pub(super) const SUBHEADERS: usize = KEYS + 2 * 256;
const SUBHEADER: usize = 8;
/// Keys are subheader offsets: multiples of the subheader size.
const KEY_STRIDE: usize = 8;
/// Format 10's header, before its glyph array.
const TRIMMED_ARRAY: usize = 20;
/// The size of the Unicode code space, which format 10 ranges stay within.
const UNICODE_CODES: u64 = 0x0011_0000;

pub(super) fn validate_two(mut r: Cursor<'_>, glyphs: usize, work: &mut Work<'_>) -> Result<()> {
    r.skip(KEYS)?;
    let mut largest = 0;
    for _ in 0..256 {
        let key = usize::from(r.u16()?);
        if !key.is_multiple_of(KEY_STRIDE) {
            return Err(r.error("unaligned cmap subheader key"));
        }
        largest = largest.max(key / KEY_STRIDE);
    }
    work.entries(largest + 1, r.position())?;
    work.work(256, r.position())?;
    let data = r.data();
    let headers = available(&r, largest + 1, SUBHEADER, work)?;
    for index in 0..headers {
        let mut header = r.cursor_at(SUBHEADERS + SUBHEADER * index)?;
        let first = usize::from(header.u16()?);
        let count = usize::from(header.u16()?);
        let delta = header.u16()?;
        let relative_at = header.position();
        let relative = usize::from(header.u16()?);
        if first + count > 256 || !relative.is_multiple_of(2) {
            return Err(header.error("invalid cmap subheader range"));
        }
        work.work(count, header.position())?;
        // idRangeOffset counts from its own word.
        let start = SUBHEADERS + SUBHEADER * index + 6 + relative;
        if start > data.len() {
            work.repair(
                repairs::CMAP_TRUNCATED,
                "missing cmap subheader glyph array",
                relative_at,
            )?;
            continue;
        }
        let mut glyph_array = r.cursor_at(start)?;
        for _ in 0..available(&glyph_array, count, 2, work)? {
            let raw = glyph_array.u16()?;
            let glyph = if raw == 0 { 0 } else { raw.wrapping_add(delta) };
            if usize::from(glyph) >= glyphs {
                return Err(glyph_array.error("invalid cmap glyph"));
            }
        }
    }
    Ok(())
}

/// The glyph of a two-byte (or single-byte) `code` in format 2 `data`;
/// `None` if a needed word is missing.
pub(super) fn lookup_two(data: &[u8], code: u16) -> Option<u16> {
    let [high, low] = code.to_be_bytes().map(usize::from);
    // A high byte with key 0 is a single-byte code itself; other high
    // bytes select the subheader for their second byte.
    let key = usize::from(u16_at(data, KEYS + 2 * if high == 0 { low } else { high })?);
    if (high == 0) != (key == 0) {
        return Some(0);
    }
    let at = SUBHEADERS + key;
    let first = usize::from(u16_at(data, at)?);
    let count = usize::from(u16_at(data, at + 2)?);
    let Some(index) = low.checked_sub(first).filter(|&i| i < count) else {
        return Some(0);
    };
    let delta = u16_at(data, at + 4)?;
    let relative = usize::from(u16_at(data, at + 6)?);
    let glyph = u16_at(data, at + 6 + relative + 2 * index)?;
    Some(if glyph == 0 {
        0
    } else {
        glyph.wrapping_add(delta)
    })
}

pub(super) fn validate_ten(
    mut r: Cursor<'_>,
    glyphs: usize,
    work: &mut Work<'_>,
) -> Result<Layout> {
    r.skip(12)?;
    let first = r.u32()?;
    let count = r.u32()?;
    if u64::from(first) + u64::from(count) > UNICODE_CODES {
        return Err(r.error("invalid cmap Unicode range"));
    }
    let entries = u32_to_usize(count);
    work.entries(entries, r.position())?;
    work.work(entries, r.position())?;
    for _ in 0..available(&r, entries, 2, work)? {
        if usize::from(r.u16()?) >= glyphs {
            return Err(r.error("invalid cmap glyph"));
        }
    }
    Ok(Layout::Trimmed {
        first,
        count,
        array: TRIMMED_ARRAY,
    })
}

#[cfg(test)]
mod tests {
    use super::{SUBHEADERS, lookup_two};

    #[test]
    fn format_two_selects_single_bytes_and_subheaders() {
        let mut data = vec![0; SUBHEADERS];
        // High byte 0x81 uses subheader 1; others are single bytes.
        data[6 + 2 * 0x81..6 + 2 * 0x81 + 2].copy_from_slice(&8_u16.to_be_bytes());
        // Subheader 0 maps codes 0x41-0x42 to the first two glyph words;
        // subheader 1 maps second byte 0x40 to the third, plus delta 2.
        // Each idRangeOffset counts from its own word.
        for words in [[0x41, 2, 0, 10], [0x40, 1, 2, 6]] {
            data.extend(words.iter().flat_map(|w: &u16| w.to_be_bytes()));
        }
        data.extend([5_u16, 6, 7].iter().flat_map(|w| w.to_be_bytes()));
        assert_eq!(lookup_two(&data, 0x41), Some(5));
        assert_eq!(lookup_two(&data, 0x42), Some(6));
        assert_eq!(lookup_two(&data, 0x43), Some(0));
        assert_eq!(lookup_two(&data[..520], 0x41), None);
        assert_eq!(lookup_two(&data, 0x8140), Some(9));
        // 0x81 alone is a lead byte, not a character.
        assert_eq!(lookup_two(&data, 0x81), Some(0));
        assert_eq!(lookup_two(&data, 0x8240), Some(0));
    }
}
