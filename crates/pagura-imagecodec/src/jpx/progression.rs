//! Precinct reference-grid positions and bounded progression changes.
//!
//! T.800 B.2–B.7 and B.12 define the geometry and progression volumes.

use pagura_core::bytes::Cursor;
use pagura_core::convert::{u32_to_usize, usize_to_u64};
use pagura_core::{Error, Result};

use crate::budget::Budget;

use super::{area, syntax};
#[derive(Clone, Copy)]
pub(super) struct Precinct {
    pub(super) bounds: [u32; 4],
    pub(super) position: [u64; 2],
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

/// The most resolution levels a tile-component has: 32 decomposition
/// levels and the lowest resolution (T.800 A.6.1, Table A.15).
pub(super) const MAX_RESOLUTIONS: usize = 33;

/// The order in which packets follow each other (T.800 B.12, Table A.16):
/// by layer, resolution, component and position, in the order the name
/// lists them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum ProgressionOrder {
    /// Layer, resolution, component, position.
    #[default]
    Lrcp,
    /// Resolution, layer, component, position.
    Rlcp,
    /// Resolution, position, component, layer.
    Rpcl,
    /// Position, component, resolution, layer.
    Pcrl,
    /// Component, position, resolution, layer.
    Cprl,
}

impl ProgressionOrder {
    /// Whether positions on the reference grid order the packets, which
    /// needs power-of-two component sampling (T.800 B.12.1.3, B.12.1.4).
    pub(super) const fn is_spatial(self) -> bool {
        matches!(self, Self::Rpcl | Self::Pcrl)
    }
}

impl TryFrom<u8> for ProgressionOrder {
    type Error = u8;

    fn try_from(code: u8) -> std::result::Result<Self, u8> {
        match code {
            0 => Ok(Self::Lrcp),
            1 => Ok(Self::Rlcp),
            2 => Ok(Self::Rpcl),
            3 => Ok(Self::Pcrl),
            4 => Ok(Self::Cprl),
            _ => Err(code),
        }
    }
}

/// One progression volume of a POC marker segment (T.800 A.6.6).
#[derive(Clone, Copy)]
pub(super) struct Change {
    pub(super) resolution_start: usize,
    pub(super) component_start: usize,
    pub(super) layer_end: usize,
    pub(super) resolution_end: usize,
    pub(super) component_end: usize,
    pub(super) order: ProgressionOrder,
}

pub(super) fn changes(
    fields: &mut Cursor<'_>,
    components: usize,
    layers: usize,
    budget: &mut Budget,
) -> Result<Vec<Change>> {
    let unit = if components <= 256 { 7 } else { 9 };
    if fields.at_end() || !fields.remaining().is_multiple_of(unit) {
        return Err(fields.error("invalid JPEG 2000 progression change length"));
    }
    let count = fields.remaining() / unit;
    let mut result = budget.with_capacity(count)?;
    while !fields.at_end() {
        let at = fields.position();
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
        // T.800 Table A.32 assigns zero the end-index value 256, including the wide field.
        if component_end == 0 {
            component_end = 256;
        }
        let order = ProgressionOrder::try_from(fields.u8()?)
            .ok()
            .filter(|_| {
                resolution_start < resolution_end
                    && resolution_end <= MAX_RESOLUTIONS
                    && component_start < component_end
                    && component_end <= components
                    && layer_end != 0
                    && layer_end <= layers
            })
            .ok_or_else(|| syntax("invalid JPEG 2000 progression change range", at))?;
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
    pub(super) layer: usize,
    pub(super) resolution: usize,
    pub(super) component: usize,
    pub(super) precinct: usize,
    pub(super) id: usize,
    pub(super) position: [u64; 2],
}

impl Packet {
    /// The packet's place in `order`. Spatial orders use the precinct's
    /// reference-grid position (T.800 B.12.1.3 to B.12.1.5).
    fn key(self, order: ProgressionOrder) -> [u64; 5] {
        let [x, y] = self.position;
        let [l, r, c, p] =
            [self.layer, self.resolution, self.component, self.precinct].map(usize_to_u64);
        match order {
            ProgressionOrder::Lrcp => [l, r, c, p, 0],
            ProgressionOrder::Rlcp => [r, l, c, p, 0],
            ProgressionOrder::Rpcl => [r, y, x, c, l],
            ProgressionOrder::Pcrl => [y, x, c, r, l],
            ProgressionOrder::Cprl => [c, y, x, r, l],
        }
    }
}

pub(super) fn order(
    packets: &[Packet],
    changes: &[Change],
    default: ProgressionOrder,
    visited: &[bool],
    budget: &mut Budget,
) -> Result<Vec<Packet>> {
    let mut included = budget.filled(packets.len(), false)?;
    let mut result = budget.with_capacity(packets.len())?;
    let fallback = [Change {
        resolution_start: 0,
        component_start: 0,
        layer_end: usize::MAX,
        resolution_end: MAX_RESOLUTIONS,
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
        budget.work_product(count, u32_to_usize(count.bit_width()))?;
        result[start..].sort_unstable_by_key(|p| p.key(change.order));
    }
    Ok(result)
}
