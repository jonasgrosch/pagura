//! The choice of formats (ADR 0004 §5–§6): from the innermost buffer
//! outwards by the error bounds of their stores, then widened to F64 from
//! the outermost buffer inwards.

use super::{Buffer, Survey};
use crate::format::Format;
use pagura_core::{Error, Result};
use pagura_raster::DeviceRect;

/// `π` of the F32 bound: source encoding and six roundings relative to the
/// opacity, in levels.
const PI: f64 = 1.0 / 65_536.0 + 255.0 * 6.0 / 16_777_216.0;

/// `ρ` of the F32 bound: one rounding below 256 levels.
const RHO: f64 = 1.0 / 131_072.0;

/// The margin for f64 source evaluation, in levels.
pub(super) const MARGIN: f64 = 1.0 / 4096.0;

/// The error of one f64 store of a non-amplifying operation, in levels.
pub(super) const F64_STORE: f64 = 1.0 / 1_099_511_627_776.0;

/// The F32 admission limit, in levels.
const F32_LIMIT: f64 = 0.5 - 1.0 / 4096.0;

/// The room an N16 buffer leaves for dynamic stores, in words.
const N16_ROOM: f64 = 0.5 - 1.0 / 68_719_476_736.0;

/// The most dynamic stores whose footprints N16 admission compares.
pub(super) const FOOTPRINTS: usize = 4096;

/// Whether an N16 page admits `buffer`'s dynamic stores: at most one at
/// any pixel, each injecting less than the room an N16 pixel leaves
/// (ADR 0004 §6). Footprints are page-level device bounds, so the answer
/// does not depend on the region. Children and masks are resolved first.
pub(super) fn n16_admits(buffer: &Buffer, resolved: &[Option<(Format, f64)>]) -> bool {
    if buffer.dynamics.len() != buffer.dynamic || buffer.dynamic > FOOTPRINTS {
        return false;
    }
    let bound = |node: usize| {
        resolved
            .get(node)
            .copied()
            .flatten()
            .map_or(f64::INFINITY, |(_, bound)| bound)
    };
    let fits = buffer.dynamics.iter().all(|store| {
        let child = store
            .child
            .map_or(0.0, |c| bound(c) * (1.0 + 256.0 / 255.0) + F64_STORE);
        let mask = store.mask.map_or(0.0, |m| 256.0 / 255.0 * bound(m));
        257.0 * (store.eta + child + mask) <= N16_ROOM
    });
    let mut rects: Vec<DeviceRect> = buffer
        .dynamics
        .iter()
        .map(|store| store.rect)
        .filter(|rect| !rect.is_empty())
        .collect();
    fits && disjoint(&mut rects)
}

/// Whether no two rectangles share a pixel.
fn disjoint(rects: &mut [DeviceRect]) -> bool {
    rects.sort_unstable_by_key(|r| r.x0);
    let mut active: Vec<DeviceRect> = Vec::new();
    for rect in rects.iter() {
        active.retain(|other| other.x1 > rect.x0);
        if active
            .iter()
            .any(|other| other.y0 < rect.y1 && rect.y0 < other.y1)
        {
            return false;
        }
        active.push(*rect);
    }
    true
}

impl Survey<'_> {
    /// Chooses formats from the innermost buffer outwards and records each
    /// buffer's error bound in levels.
    pub(super) fn resolve(
        &self,
        node: usize,
        resolved: &mut Vec<Option<(Format, f64)>>,
        depth: usize,
    ) -> Result<(Format, f64)> {
        if let Some(Some(done)) = resolved.get(node) {
            return Ok(*done);
        }
        if depth > self.inputs.depth.saturating_mul(2) {
            return Err(Error::limit("compositing depth exceeded"));
        }
        let buffer = &self.buffers[node];
        let mut injected = 0.0;
        for &(child, count) in &buffer.children {
            let (_, bound) = self.resolve(child, resolved, depth + 1)?;
            injected += (bound * (1.0 + 256.0 / 255.0) + F64_STORE) * count as f64;
        }
        for &(mask, count) in &buffer.masks {
            let (_, bound) = self.resolve(mask, resolved, depth + 1)?;
            injected += 256.0 / 255.0 * bound * count as f64;
        }
        let stores = buffer.stores as f64;
        let wide = (stores * F64_STORE + MARGIN + injected, Format::F64);
        let n16 = buffer.page && n16_admits(buffer, resolved);
        let f32_bound =
            PI + RHO * stores.min(1.0 / buffer.theta + buffer.dynamic as f64) + injected;
        let (bound, format) = if buffer.forced {
            wide
        } else if n16 {
            // The page's bound is never used as a child's.
            (0.5, Format::N16)
        } else if f32_bound <= F32_LIMIT {
            (f32_bound + MARGIN, Format::F32)
        } else {
            wide
        };
        if let Some(slot) = resolved.get_mut(node) {
            *slot = Some((format, bound));
        }
        Ok((format, bound))
    }

    /// Widens every child and mask of an F64 buffer to F64, from the
    /// outermost buffer inwards: no narrow child of an F64 parent is proven.
    pub(super) fn widen(&self, root: usize, resolved: &mut [Option<(Format, f64)>]) {
        // 1: visited while narrow, 2: visited while F64.
        let mut seen = vec![0_u8; self.buffers.len()];
        let mut pending = vec![(root, false)];
        while let Some((node, parent_wide)) = pending.pop() {
            let Some(Some((format, _))) = resolved.get_mut(node) else {
                continue;
            };
            if parent_wide {
                *format = Format::F64;
            }
            let wide = *format == Format::F64;
            let mark = if wide { 2 } else { 1 };
            if seen[node] >= mark {
                continue;
            }
            seen[node] = mark;
            let buffer = &self.buffers[node];
            for &(child, _) in buffer.children.iter().chain(&buffer.masks) {
                pending.push((child, wide));
            }
        }
    }
}
