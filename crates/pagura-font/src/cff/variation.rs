//! CFF2 ItemVariationStore at normalized default coordinates (OpenType CFF2).
use crate::read::{Read, bad, cap};
use pagura_core::{Limits, Result};

pub(super) fn parse(mut r: Read<'_>, limits: &Limits) -> Result<Vec<Vec<f64>>> {
    let length = usize::from(r.u16()?);
    let base = r.at();
    let data = if length == 65535 {
        r.data
            .get(r.pos..)
            .ok_or_else(|| bad(base, "truncated variation store"))?
    } else {
        r.take(length)?
    };
    let mut store = Read::new(data, base);
    if store.u16()? != 1 {
        return Err(bad(store.at(), "invalid ItemVariationStore format"));
    }
    let region_offset = store.offset()?;
    let count = usize::from(store.u16()?);
    cap(count, limits.value_nodes, store.at())?;
    let mut offsets = Vec::with_capacity(count);
    for _ in 0..count {
        offsets.push(store.offset()?);
    }
    let mut regions = store.tail(region_offset)?;
    let axes = usize::from(regions.u16()?);
    let region_count = usize::from(regions.u16()?);
    cap(
        region_count,
        limits
            .value_nodes
            .min(limits.decoded_bytes / size_of::<f64>()),
        regions.at(),
    )?;
    let work = axes
        .checked_mul(region_count)
        .ok_or_else(|| bad(regions.at(), "variation region count overflow"))?;
    cap(
        work,
        limits.operations.min(limits.value_nodes),
        regions.at(),
    )?;
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
                return Err(bad(regions.at(), "invalid variation region axis"));
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
    let mut total = 0_usize;
    for offset in offsets {
        let mut item = store.tail(offset)?;
        let items = usize::from(item.u16()?);
        let words = item.u16()?;
        let regions = usize::from(item.u16()?);
        let long = words & 0x8000 != 0;
        let words = usize::from(words & 0x7fff);
        if words > regions {
            return Err(bad(item.at(), "invalid variation delta count"));
        }
        total = total.saturating_add(regions);
        cap(total, limits.value_nodes.min(limits.operations), item.at())?;
        let mut coefficients = Vec::with_capacity(regions);
        for _ in 0..regions {
            let index = usize::from(item.u16()?);
            coefficients.push(
                *scalars
                    .get(index)
                    .ok_or_else(|| bad(item.at(), "variation region index out of range"))?,
            );
        }
        let row = words
            .checked_mul(if long { 4 } else { 2 })
            .and_then(|bytes| bytes.checked_add((regions - words) * if long { 2 } else { 1 }))
            .ok_or_else(|| bad(item.at(), "variation delta size overflow"))?;
        item.skip(
            items
                .checked_mul(row)
                .ok_or_else(|| bad(item.at(), "variation delta size overflow"))?,
        )?;
        result.push(coefficients);
    }
    Ok(result)
}
