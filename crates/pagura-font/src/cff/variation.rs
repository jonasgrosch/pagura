//! CFF2 ItemVariationStore at normalized default coordinates (OpenType CFF2).
use crate::budget::Work;
use crate::read::{bad, here};
use pagura_core::bytes::Cursor;
use pagura_core::convert::u32_to_usize;
use pagura_core::{Position, Result};

pub(super) fn parse(mut r: Cursor<'_>, work: &mut Work<'_>) -> Result<Vec<Vec<f64>>> {
    let length = usize::from(r.u16()?);
    let mut store = if length == 65535 {
        r.take_cursor(r.remaining())?
    } else {
        r.take_cursor(length)?
    };
    if store.u16()? != 1 {
        return Err(bad(here(&store), "invalid ItemVariationStore format"));
    }
    let region_offset = u32_to_usize(store.u32()?);
    let count = usize::from(store.u16()?);
    work.entries(count, Position::Buffer(here(&store)))?;
    let mut offsets = Vec::with_capacity(count);
    for _ in 0..count {
        offsets.push(u32_to_usize(store.u32()?));
    }
    let mut regions = store.cursor_at(region_offset)?;
    let axes = usize::from(regions.u16()?);
    let region_count = usize::from(regions.u16()?);
    work.entries(region_count, Position::Buffer(here(&regions)))?;
    let cells = axes
        .checked_mul(region_count)
        .ok_or_else(|| crate::read::limit(here(&regions), "variation region count overflow"))?;
    work.work(cells, Position::Buffer(here(&regions)))?;
    let mut scalars = Vec::with_capacity(region_count);
    for _ in 0..region_count {
        let mut scalar = 1.0;
        for _ in 0..axes {
            let start = regions.i16()?;
            let peak = regions.i16()?;
            let end = regions.i16()?;
            if !(-16384..=16384).contains(&start)
                || !(-16384..=16384).contains(&peak)
                || !(-16384..=16384).contains(&end)
                || start > peak
                || peak > end
            {
                return Err(bad(here(&regions), "invalid variation region axis"));
            }
            // Peak zero and supports crossing zero do not participate in
            // the scalar, per the Item Variation Store interpolation rules.
            if peak != 0 && !(start < 0 && end > 0) {
                scalar = 0.0;
            }
        }
        scalars.push(scalar);
    }
    let mut result = Vec::with_capacity(count);
    for offset in offsets {
        let mut item = store.cursor_at(offset)?;
        let items = usize::from(item.u16()?);
        let words = item.u16()?;
        let regions = usize::from(item.u16()?);
        let long = words & 0x8000 != 0;
        let words = usize::from(words & 0x7fff);
        if words > regions {
            return Err(bad(here(&item), "invalid variation delta count"));
        }
        work.entries(regions, Position::Buffer(here(&item)))?;
        let mut coefficients = Vec::with_capacity(regions);
        for _ in 0..regions {
            let index = usize::from(item.u16()?);
            coefficients.push(
                *scalars
                    .get(index)
                    .ok_or_else(|| bad(here(&item), "variation region index out of range"))?,
            );
        }
        let row = words
            .checked_mul(if long { 4 } else { 2 })
            .and_then(|bytes| bytes.checked_add((regions - words) * if long { 2 } else { 1 }))
            .ok_or_else(|| crate::read::limit(here(&item), "variation delta size overflow"))?;
        item.skip(
            items
                .checked_mul(row)
                .ok_or_else(|| crate::read::limit(here(&item), "variation delta size overflow"))?,
        )?;
        result.push(coefficients);
    }
    Ok(result)
}
