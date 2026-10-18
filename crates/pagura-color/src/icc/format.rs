//! Profile admission: ICC.1:2010 7.2–7.4, 8.2–8.7 and 9.2.15/34.

use super::{At, Budget, D50, xyz};
use pagura_core::bytes::Cursor;
use pagura_core::{Error, Result};
use std::collections::BTreeMap;

/// Tag bodies by signature, each a cursor over exactly its declared extent.
pub(super) type Tags<'a> = BTreeMap<[u8; 4], Cursor<'a>>;

/// Profile header size before the tag count (7.2).
const HEADER: usize = 128;
/// One tag table entry: signature, offset and size (7.3).
const TAG_ENTRY: usize = 12;
/// Estimated working storage of one parsed tag table entry.
pub(super) const TAG_BYTES: usize = 96;

pub(super) fn header<'a>(
    profile: &Cursor<'a>,
    components: usize,
    budget: &mut Budget,
) -> Result<Tags<'a>> {
    let length = profile.data().len();
    if usize::try_from(profile.u32_at(0)?).ok() != Some(length) || profile.bytes(36, 4)? != b"acsp"
    {
        return Err(Error::syntax("invalid ICC profile header or size"));
    }
    if !matches!(profile.bytes(12, 4)?, b"scnr" | b"mntr" | b"prtr" | b"spac") {
        return Err(Error::syntax("invalid PDF ICC profile class"));
    }
    let actual = match profile.bytes(16, 4)? {
        b"GRAY" => 1,
        b"RGB " | b"Lab " => 3,
        b"CMYK" => 4,
        _ => return Err(Error::syntax("invalid PDF ICC data colour space")),
    };
    if actual != components {
        return Err(Error::syntax("ICC profile N differs from header"));
    }
    if !matches!(profile.bytes(20, 4)?, b"XYZ " | b"Lab ") {
        return Err(Error::syntax("invalid ICC PCS signature"));
    }
    let version = profile.bytes(8, 4)?;
    if version[..2].iter().any(|b| b >> 4 > 9 || b & 15 > 9) || version[2..] != [0, 0] {
        return Err(Error::syntax("invalid ICC version encoding"));
    }
    let v4 = version[0] == 4;
    // ICC.1:2010 validates v4 metadata. Legacy v2 metadata conventions are not
    // inferred from v4 rules; its transforms retain their explicit encodings.
    if v4 {
        validate_v4(profile)?;
    }
    let mut table = profile.cursor_at(HEADER)?;
    let count =
        usize::try_from(table.u32()?).map_err(|_| Error::limit("ICC tag count overflow"))?;
    budget.work.charge(count)?;
    budget.bytes.charge_product(count, TAG_BYTES)?;
    let end = count
        .checked_mul(TAG_ENTRY)
        .and_then(|v| v.checked_add(HEADER + 4))
        .ok_or(Error::limit("ICC tag table overflow"))?;
    let mut entries = table.take_cursor(end - HEADER - 4)?;
    let mut tags = Tags::new();
    let mut extents = Vec::new();
    extents
        .try_reserve_exact(count)
        .map_err(|_| Error::limit("ICC tag table allocation failed"))?;
    for _ in 0..count {
        let name = entries.take_array::<4>()?;
        let offset =
            usize::try_from(entries.u32()?).map_err(|_| Error::limit("ICC offset overflow"))?;
        let length =
            usize::try_from(entries.u32()?).map_err(|_| Error::limit("ICC tag size overflow"))?;
        if offset < end || !offset.is_multiple_of(4) || length < 8 {
            return Err(Error::syntax("invalid ICC tag extent").at(entries.position()));
        }
        if tags.insert(name, profile.sub(offset, length)?).is_some() {
            return Err(Error::syntax("duplicate ICC tag").at(entries.position()));
        }
        extents.push((offset, length));
    }
    validate_extents(profile, end, &mut extents, v4)?;
    Ok(tags)
}

fn validate_v4(profile: &Cursor<'_>) -> Result<()> {
    let mut date = profile.cursor_at(24)?;
    let [year, month, day, hour, minute, second] = [
        date.u16()?,
        date.u16()?,
        date.u16()?,
        date.u16()?,
        date.u16()?,
        date.u16()?,
    ];
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        4 | 6 | 9 | 11 => 30,
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        _ => 0,
    };
    if year == 0 || day == 0 || day > days || hour > 23 || minute > 59 || second > 59 {
        return Err(Error::syntax("invalid ICC creation date"));
    }
    if profile.u32_at(60)? & !15 != 0 || profile.u32_at(64)? > 3 {
        return Err(Error::syntax("invalid ICC attributes or rendering intent"));
    }
    let mut illuminant = profile.cursor_at(68)?;
    for white in [63190, 65536, 54061] {
        if illuminant.u32()? != white {
            return Err(Error::syntax("ICC PCS illuminant is not D50"));
        }
    }
    zeros(profile, 100, 28)?;
    let id = profile.bytes(84, 16)?;
    if id.iter().any(|&v| v != 0) {
        let length = profile.data().len();
        let mut digest = pagura_crypto::Md5::default();
        digest.update(profile.bytes(0, 44)?);
        digest.update(&[0; 4]);
        digest.update(profile.bytes(48, 16)?);
        digest.update(&[0; 4]);
        digest.update(profile.bytes(68, 16)?);
        digest.update(&[0; 16]);
        digest.update(profile.bytes(100, length - 100)?);
        if digest.finalize() != id {
            return Err(Error::syntax("ICC profile ID does not match profile data"));
        }
    }
    Ok(())
}

fn validate_extents(
    profile: &Cursor<'_>,
    mut cursor: usize,
    extents: &mut Vec<(usize, usize)>,
    tight: bool,
) -> Result<()> {
    extents.sort_unstable();
    extents.dedup(); // Exact sharing is legal, including shared TRC bodies (7.4).
    for &(offset, length) in extents.iter() {
        if offset < cursor || (tight && offset != cursor) {
            return Err(Error::syntax("overlapping or unpacked ICC tag data"));
        }
        cursor = offset
            .checked_add(length)
            .ok_or(Error::limit("ICC extent overflow"))?;
        if tight {
            let padding = (4 - cursor % 4) % 4;
            zeros(profile, cursor, padding)?;
            cursor += padding;
        }
    }
    if tight && cursor != profile.data().len() {
        return Err(Error::syntax("ICC profile has trailing tag data"));
    }
    Ok(())
}

/// Requires `length` reserved or padding bytes at `offset` to be zero.
pub(super) fn zeros(data: &Cursor<'_>, offset: usize, length: usize) -> Result<()> {
    let bytes = data.sub(offset, length)?;
    if bytes.data().iter().any(|&v| v != 0) {
        return Err(bytes.error("nonzero ICC reserved or padding bytes"));
    }
    Ok(())
}

pub(super) fn required<'t, 'a>(tags: &'t Tags<'a>, name: &[u8; 4]) -> Result<&'t Cursor<'a>> {
    tags.get(name)
        .ok_or(Error::syntax("missing required ICC tag"))
}

pub(super) fn model_tags(
    profile: &Cursor<'_>,
    tags: &Tags<'_>,
    budget: &mut Budget,
) -> Result<[f64; 3]> {
    let white = xyz(required(tags, b"wtpt")?)?;
    let class = profile.bytes(12, 4)?;
    let space = profile.bytes(16, 4)?;
    if profile.u8_at(8)? == 4 {
        required(tags, b"desc")?;
        required(tags, b"cprt")?;
        if class == b"mntr" && white != D50.map(|v| (v * 65536.0).round() / 65536.0) {
            return Err(Error::syntax("ICC display media white is not PCS D50"));
        }
        let gray = tags.contains_key(b"kTRC") && space == b"GRAY";
        let matrix = space == b"RGB "
            && profile.bytes(20, 4)? == b"XYZ "
            && [b"rXYZ", b"gXYZ", b"bXYZ", b"rTRC", b"gTRC", b"bTRC"]
                .iter()
                .all(|v| tags.contains_key(*v));
        match class {
            b"scnr" if !gray && !matrix => {
                required(tags, b"A2B0")?;
            }
            b"mntr" if !gray && !matrix => {
                required(tags, b"A2B0")?;
                required(tags, b"B2A0")?;
            }
            b"prtr" if !gray => {
                for tag in [
                    b"A2B0", b"A2B1", b"A2B2", b"B2A0", b"B2A1", b"B2A2", b"gamt",
                ] {
                    required(tags, tag)?;
                }
            }
            b"spac" => {
                required(tags, b"A2B0")?;
                required(tags, b"B2A0")?;
            }
            _ => {}
        }
    }
    if let Some(chad) = tags.get(b"chad") {
        budget.values(9)?;
        if chad.bytes(0, 4)? != b"sf32" || chad.data().len() != 44 {
            return Err(Error::syntax(
                "invalid ICC chromatic adaptation type or extent",
            ));
        }
        zeros(chad, 4, 4)?;
        // Fixed coefficients are integer multiples of 2^-16. A determinant
        // of three i32 factors fits i128, avoiding cancellation near singularity.
        let mut values = chad.cursor_at(8)?;
        let mut m = [0_i128; 9];
        for v in &mut m {
            *v = i128::from(values.i32()?);
        }
        let determinant = m[0] * (m[4] * m[8] - m[5] * m[7]) - m[1] * (m[3] * m[8] - m[5] * m[6])
            + m[2] * (m[3] * m[7] - m[4] * m[6]);
        if determinant == 0 {
            return Err(Error::syntax("singular ICC chromatic adaptation matrix"));
        }
        // Profile PCS data is already adapted (6.3.1, 9.2.15); never apply chad twice.
    }
    Ok(white)
}
