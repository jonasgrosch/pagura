//! Profile admission: ICC.1:2010 7.2–7.4, 8.2–8.7 and 9.2.15/34.

use super::{Budget, D50, slice, u16_at, u32_at, xyz};
use pagura_core::{Error, Limits, Result};
use std::collections::BTreeMap;

pub(super) type Tags<'a> = BTreeMap<[u8; 4], &'a [u8]>;

pub(super) fn header(data: &[u8], components: usize, limits: Limits) -> Result<Tags<'_>> {
    if usize::try_from(u32_at(data, 0)?).ok() != Some(data.len()) || slice(data, 36, 4)? != b"acsp"
    {
        return Err(Error::syntax("invalid ICC profile header or size"));
    }
    if !matches!(slice(data, 12, 4)?, b"scnr" | b"mntr" | b"prtr" | b"spac") {
        return Err(Error::syntax("invalid PDF ICC profile class"));
    }
    let actual = match slice(data, 16, 4)? {
        b"GRAY" => 1,
        b"RGB " | b"Lab " => 3,
        b"CMYK" => 4,
        _ => return Err(Error::syntax("invalid PDF ICC data colour space")),
    };
    if actual != components {
        return Err(Error::syntax("ICC profile N differs from header"));
    }
    if !matches!(slice(data, 20, 4)?, b"XYZ " | b"Lab ") {
        return Err(Error::syntax("invalid ICC PCS signature"));
    }
    let version = slice(data, 8, 4)?;
    if version[..2].iter().any(|b| b >> 4 > 9 || b & 15 > 9) || version[2..] != [0, 0] {
        return Err(Error::syntax("invalid ICC version encoding"));
    }
    // ICC.1:2010 validates v4 metadata. Legacy v2 metadata conventions are not
    // inferred from v4 rules; its transforms retain their explicit encodings.
    if version[0] == 4 {
        validate_v4(data)?;
    }
    let count =
        usize::try_from(u32_at(data, 128)?).map_err(|_| Error::limit("ICC tag count overflow"))?;
    if count > limits.value_nodes || count > limits.decoded_bytes / 96 {
        return Err(Error::limit("ICC tag budget exceeded"));
    }
    let end = count
        .checked_mul(12)
        .and_then(|v| v.checked_add(132))
        .ok_or(Error::limit("ICC tag table overflow"))?;
    slice(data, 0, end)?;
    let mut tags = Tags::new();
    let mut extents = Vec::with_capacity(count);
    for i in 0..count {
        let entry = 132 + i * 12;
        let name = slice(data, entry, 4)?
            .try_into()
            .map_err(|_| Error::syntax("invalid ICC tag signature"))?;
        let offset = usize::try_from(u32_at(data, entry + 4)?)
            .map_err(|_| Error::limit("ICC offset overflow"))?;
        let length = usize::try_from(u32_at(data, entry + 8)?)
            .map_err(|_| Error::limit("ICC tag size overflow"))?;
        if offset < end || !offset.is_multiple_of(4) || length < 8 {
            return Err(Error::syntax("invalid ICC tag extent"));
        }
        if tags.insert(name, slice(data, offset, length)?).is_some() {
            return Err(Error::syntax("duplicate ICC tag"));
        }
        extents.push((offset, length));
    }
    validate_extents(data, end, &mut extents, version[0] == 4)?;
    Ok(tags)
}

fn validate_v4(data: &[u8]) -> Result<()> {
    let [year, month, day, hour, minute, second] = [24, 26, 28, 30, 32, 34]
        .map(|at| u16_at(data, at))
        .into_iter()
        .collect::<Result<Vec<_>>>()?
        .try_into()
        .map_err(|_| Error::syntax("invalid ICC date"))?;
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
    if u32_at(data, 60)? & !15 != 0 || u32_at(data, 64)? > 3 {
        return Err(Error::syntax("invalid ICC attributes or rendering intent"));
    }
    for (i, white) in [63190, 65536, 54061].into_iter().enumerate() {
        if u32_at(data, 68 + 4 * i)? != white {
            return Err(Error::syntax("ICC PCS illuminant is not D50"));
        }
    }
    zeros(slice(data, 100, 28)?)?;
    let id = slice(data, 84, 16)?;
    if id.iter().any(|&v| v != 0) {
        let mut digest = pagura_crypto::Md5::default();
        digest.update(slice(data, 0, 44)?);
        digest.update(&[0; 4]);
        digest.update(slice(data, 48, 16)?);
        digest.update(&[0; 4]);
        digest.update(slice(data, 68, 16)?);
        digest.update(&[0; 16]);
        digest.update(slice(data, 100, data.len() - 100)?);
        if digest.finalize() != id {
            return Err(Error::syntax("ICC profile ID does not match profile data"));
        }
    }
    Ok(())
}

fn validate_extents(
    data: &[u8],
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
            zeros(slice(data, cursor, padding)?)?;
            cursor += padding;
        }
    }
    if tight && cursor != data.len() {
        return Err(Error::syntax("ICC profile has trailing tag data"));
    }
    Ok(())
}

pub(super) fn zeros(data: &[u8]) -> Result<()> {
    if data.iter().any(|&v| v != 0) {
        return Err(Error::syntax("nonzero ICC reserved or padding bytes"));
    }
    Ok(())
}

pub(super) fn required<'a>(tags: &Tags<'a>, name: &[u8]) -> Result<&'a [u8]> {
    tags.get(name)
        .copied()
        .ok_or(Error::syntax("missing required ICC tag"))
}

pub(super) fn model_tags(data: &[u8], tags: &Tags<'_>, budget: &mut Budget) -> Result<[f64; 3]> {
    let white = xyz(required(tags, b"wtpt")?)?;
    if data[8] == 4 {
        required(tags, b"desc")?;
        required(tags, b"cprt")?;
        if slice(data, 12, 4)? == b"mntr" && white != D50.map(|v| (v * 65536.0).round() / 65536.0) {
            return Err(Error::syntax("ICC display media white is not PCS D50"));
        }
        let gray = tags.contains_key(b"kTRC") && slice(data, 16, 4)? == b"GRAY";
        let matrix = slice(data, 16, 4)? == b"RGB "
            && slice(data, 20, 4)? == b"XYZ "
            && [b"rXYZ", b"gXYZ", b"bXYZ", b"rTRC", b"gTRC", b"bTRC"]
                .iter()
                .all(|v| tags.contains_key(*v));
        match slice(data, 12, 4)? {
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
        budget.charge(9)?;
        if slice(chad, 0, 4)? != b"sf32" || chad.len() != 44 {
            return Err(Error::syntax(
                "invalid ICC chromatic adaptation type or extent",
            ));
        }
        zeros(slice(chad, 4, 4)?)?;
        // Fixed coefficients are integer multiples of 2^-16. A determinant
        // of three i32 factors fits i128, avoiding cancellation near singularity.
        let mut m = [0_i128; 9];
        for (i, v) in m.iter_mut().enumerate() {
            *v = i128::from(i32::from_be_bytes(u32_at(chad, 8 + 4 * i)?.to_be_bytes()));
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
