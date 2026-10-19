//! CFF2 ItemVariationStore at normalized default coordinates (OpenType CFF2).
use crate::budget::Work;
use crate::read::{cursor, here, limit, syntax};
use pagura_core::Result;
use pagura_core::bytes::Cursor;
use pagura_core::convert::u32_to_usize;

/// A vstore length of 0xffff means "to the end of the table".
const TO_END: u16 = 0xffff;
/// The only ItemVariationStore format.
const STORE_FORMAT: u16 = 1;
/// 1.0 as F2DOT14: region coordinates lie in -1..=1.
const F2DOT14_ONE: i16 = 16384;
/// ItemVariationData wordDeltaCount: the long-words flag and the count.
const LONG_WORDS: u16 = 0x8000;
const WORD_COUNT: u16 = 0x7fff;

/// The region scalars of each ItemVariationData, at the default instance.
pub(super) fn parse(mut r: Cursor<'_>, work: &mut Work<'_>) -> Result<Vec<Vec<f64>>> {
    let length = r.u16()?;
    let base = here(&r);
    let data = if length == TO_END {
        r.take_rest()
    } else {
        r.take(usize::from(length))?
    };
    let mut store = cursor(data).with_base(base);
    if store.u16()? != STORE_FORMAT {
        return Err(store.error("invalid ItemVariationStore format"));
    }
    let region_offset = u32_to_usize(store.u32()?);
    let count = usize::from(store.u16()?);
    work.entries(count, store.position())?;
    let mut offsets = Vec::with_capacity(count);
    for _ in 0..count {
        offsets.push(u32_to_usize(store.u32()?));
    }
    let scalars = regions(store.cursor_at(region_offset)?, work)?;
    let mut result = Vec::with_capacity(count);
    for offset in offsets {
        let mut item = store.cursor_at(offset)?;
        let items = usize::from(item.u16()?);
        let words = item.u16()?;
        let regions = usize::from(item.u16()?);
        let long = words & LONG_WORDS != 0;
        let words = usize::from(words & WORD_COUNT);
        if words > regions {
            return Err(item.error("invalid variation delta count"));
        }
        work.entries(regions, item.position())?;
        let mut coefficients = Vec::with_capacity(regions);
        for _ in 0..regions {
            let index = usize::from(item.u16()?);
            coefficients.push(
                *scalars
                    .get(index)
                    .ok_or_else(|| item.error("variation region index out of range"))?,
            );
        }
        let (word, short) = if long { (4, 2) } else { (2, 1) };
        let row = words
            .checked_mul(word)
            .and_then(|bytes| bytes.checked_add((regions - words) * short))
            .and_then(|row| row.checked_mul(items))
            .ok_or_else(|| limit(item.position(), "variation delta size overflow"))?;
        item.skip(row)?;
        result.push(coefficients);
    }
    Ok(result)
}

/// The scalar of each region at the default instance: zero when any axis
/// peaks away from zero, otherwise one.
fn regions(mut r: Cursor<'_>, work: &mut Work<'_>) -> Result<Vec<f64>> {
    let axes = usize::from(r.u16()?);
    let count = usize::from(r.u16()?);
    work.entries(count, r.position())?;
    work.work(
        axes.checked_mul(count)
            .ok_or_else(|| limit(r.position(), "variation region count overflow"))?,
        r.position(),
    )?;
    let coordinate = -F2DOT14_ONE..=F2DOT14_ONE;
    let mut scalars = Vec::with_capacity(count);
    for _ in 0..count {
        let mut scalar = 1.0;
        for _ in 0..axes {
            let start = r.i16()?;
            let peak = r.i16()?;
            let end = r.i16()?;
            if ![start, peak, end].iter().all(|c| coordinate.contains(c))
                || start > peak
                || peak > end
            {
                return Err(syntax(r.position(), "invalid variation region axis"));
            }
            // Peak zero and supports crossing zero do not participate in
            // the scalar, per the Item Variation Store interpolation rules.
            if peak != 0 && !(start < 0 && end > 0) {
                scalar = 0.0;
            }
        }
        scalars.push(scalar);
    }
    Ok(scalars)
}
