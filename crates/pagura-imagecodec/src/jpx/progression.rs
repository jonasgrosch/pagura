//! Precinct reference-grid positions and bounded progression changes.
//!
//! T.800 B.2–B.7 and B.12 define the geometry and progression volumes.

use pagura_core::{Error, Result};

use crate::budget::Budget;

use super::io::syntax;
use super::io::{Reader, area};

#[derive(Clone, Copy)]
pub(super) struct Precinct {
    pub bounds: [u32; 4],
    pub position: [u64; 2],
}

pub(super) fn precincts(
    bounds: [u32; 4],
    exponents: [u8; 2],
    scale: u64,
    sampling: [u32; 2],
    tile_origin: [u32; 2],
    budget: &mut Budget,
) -> Result<Vec<Precinct>> {
    let [x0, y0, x1, y1] = bounds;
    if x0 == x1 || y0 == y1 {
        return Ok(Vec::new());
    }
    let w = 1_u32 << exponents[0];
    let h = 1_u32 << exponents[1];
    let (px0, py0, px1, py1) = (x0 / w, y0 / h, x1.div_ceil(w), y1.div_ceil(h));
    let count = area(px1 - px0, py1 - py0)?;
    let mut result = budget.with_capacity(count)?;
    for py in py0..py1 {
        for px in px0..px1 {
            let x = if px == px0 && !x0.is_multiple_of(w) {
                u64::from(tile_origin[0])
            } else {
                reference_position(px, w, scale, sampling[0])?
            };
            let y = if py == py0 && !y0.is_multiple_of(h) {
                u64::from(tile_origin[1])
            } else {
                reference_position(py, h, scale, sampling[1])?
            };
            result.push(Precinct {
                bounds: [
                    (px * w).max(x0),
                    (py * h).max(y0),
                    ((px + 1) * w).min(x1),
                    ((py + 1) * h).min(y1),
                ],
                position: [x, y],
            });
        }
    }
    Ok(result)
}

fn reference_position(index: u32, size: u32, scale: u64, sampling: u32) -> Result<u64> {
    u64::from(index)
        .checked_mul(u64::from(size))
        .and_then(|v| v.checked_mul(scale))
        .and_then(|v| v.checked_mul(u64::from(sampling)))
        .ok_or_else(|| Error::limit("JPEG 2000 precinct position overflow"))
}

pub(super) fn band_bounds(bounds: [u32; 4], orientation: u8) -> [u32; 4] {
    if orientation == 0 {
        return bounds;
    }
    let [x0, y0, x1, y1] = bounds;
    let x = |v: u32| {
        if orientation & 1 == 0 {
            v.div_ceil(2)
        } else {
            v / 2
        }
    };
    let y = |v: u32| {
        if orientation & 2 == 0 {
            v.div_ceil(2)
        } else {
            v / 2
        }
    };
    [x(x0), y(y0), x(x1), y(y1)]
}

#[derive(Clone, Copy)]
pub(super) struct Change {
    pub resolution_start: usize,
    pub component_start: usize,
    pub layer_end: usize,
    pub resolution_end: usize,
    pub component_end: usize,
    pub order: u8,
}

pub(super) fn changes(
    fields: &mut Reader<'_>,
    components: usize,
    layers: usize,
    budget: &mut Budget,
) -> Result<Vec<Change>> {
    let unit = if components <= 256 { 7 } else { 9 };
    if fields.remaining() == 0 || !fields.remaining().is_multiple_of(unit) {
        return Err(syntax(
            "invalid JPEG 2000 progression change length",
            fields.position(),
        ));
    }
    let count = fields.remaining() / unit;
    let mut result = budget.with_capacity(count)?;
    while fields.remaining() != 0 {
        let resolution_start = usize::from(fields.u8()?);
        let component_start = if components <= 256 {
            usize::from(fields.u8()?)
        } else {
            usize::from(fields.u16()?)
        };
        let layer_end = usize::from(fields.u16()?);
        let resolution_end = usize::from(fields.u8()?);
        let mut component_end = if components <= 256 {
            usize::from(fields.u8()?)
        } else {
            usize::from(fields.u16()?)
        };
        // Table A.32 assigns zero the end-index value 256, including the wide field.
        if component_end == 0 {
            component_end = 256;
        }
        let order = fields.u8()?;
        if resolution_start >= resolution_end
            || resolution_end > 33
            || component_start >= component_end
            || component_end > components
            || layer_end == 0
            || layer_end > layers
            || order > 4
        {
            return Err(syntax(
                "invalid JPEG 2000 progression change range",
                fields.position() - unit,
            ));
        }
        result.push(Change {
            resolution_start,
            component_start,
            layer_end,
            resolution_end,
            component_end,
            order,
        });
    }
    Ok(result)
}

#[derive(Clone, Copy)]
pub(super) struct Packet {
    pub layer: usize,
    pub resolution: usize,
    pub component: usize,
    pub precinct: usize,
    pub id: usize,
    pub position: [u64; 2],
}

impl Packet {
    fn key(self, order: u8) -> [u64; 5] {
        let [x, y] = self.position;
        let (l, r, c, p) = (
            self.layer as u64,
            self.resolution as u64,
            self.component as u64,
            self.precinct as u64,
        );
        match order {
            1 => [r, l, c, p, 0],
            2 => [r, y, x, c, l],
            3 => [y, x, c, r, l],
            4 => [c, y, x, r, l],
            _ => [l, r, c, p, 0],
        }
    }
}

pub(super) fn order(
    packets: &[Packet],
    changes: &[Change],
    default: u8,
    visited: &[bool],
    budget: &mut Budget,
) -> Result<Vec<Packet>> {
    let mut included = budget.filled(packets.len(), false)?;
    let mut result = budget.with_capacity(packets.len())?;
    let fallback = [Change {
        resolution_start: 0,
        component_start: 0,
        layer_end: usize::MAX,
        resolution_end: 33,
        component_end: usize::MAX,
        order: default,
    }];
    for change in if changes.is_empty() {
        &fallback[..]
    } else {
        changes
    } {
        budget.work(packets.len())?;
        let start = result.len();
        for &packet in packets {
            if !visited[packet.id]
                && !included[packet.id]
                && packet.layer < change.layer_end
                && (change.resolution_start..change.resolution_end).contains(&packet.resolution)
                && (change.component_start..change.component_end).contains(&packet.component)
            {
                included[packet.id] = true;
                result.push(packet);
            }
        }
        let count = result.len() - start;
        budget.work(
            count
                .checked_mul(
                    usize::try_from(count.bit_width())
                        .map_err(|_| Error::limit("JPEG 2000 progression work overflow"))?,
                )
                .ok_or_else(|| Error::limit("JPEG 2000 progression work overflow"))?,
        )?;
        result[start..].sort_unstable_by_key(|p| p.key(change.order));
    }
    Ok(result)
}
