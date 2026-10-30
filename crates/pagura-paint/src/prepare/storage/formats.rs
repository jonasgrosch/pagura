//! The choice of formats (ADR 0004 §5–§6): from the innermost buffer
//! outwards by the error bounds of their stores, then widened to F64 from
//! the outermost buffer inwards.

use super::{Buffer, Survey};
use crate::format::Format;
use pagura_core::{Error, Meter, Result};
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

/// The footprint comparisons the disjoint-member identity may spend on one
/// page, over all its knockout groups; past them a group knocks out.
pub(super) const APART_COMPARISONS: usize = 1 << 22;

/// Whether an N16 page admits `buffer`'s dynamic stores: at most one at
/// any pixel, each injecting less than the room an N16 pixel leaves
/// (ADR 0004 §6). Footprints are page-level device bounds, so the answer
/// does not depend on the region. Children and masks are resolved first.
pub(super) fn n16_admits(
    buffer: &Buffer,
    resolved: &[Option<(Format, f64)>],
    bytes: &mut Meter,
) -> Result<bool> {
    if buffer.dynamics.len() != buffer.dynamic || buffer.dynamic > FOOTPRINTS {
        return Ok(false);
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
    if !fits {
        return Ok(false);
    }
    let mut rects = Vec::new();
    bytes.reserve_capacity(&mut rects, buffer.dynamics.len())?;
    rects.extend(
        buffer
            .dynamics
            .iter()
            .map(|store| store.rect)
            .filter(|rect| !rect.is_empty()),
    );
    // At most FOOTPRINTS rectangles: a sweep never compares more pairs.
    let admitted = disjoint(&mut rects, |_| true, bytes)?;
    let released = rects.capacity() * size_of::<DeviceRect>();
    drop(rects);
    bytes.refund(released);
    Ok(admitted)
}

/// Whether every dynamic store of knockout `buffer` has a footprint and no
/// two share a pixel, within `comparisons` (false past them). Footprints are
/// page-level device bounds, so the answer does not depend on the region.
pub(super) fn disjoint_members(
    buffer: &Buffer,
    comparisons: &mut usize,
    bytes: &mut Meter,
) -> Result<bool> {
    let footprints = &buffer.footprints;
    if buffer.dynamics.len() != buffer.dynamic
        || footprints.len() != buffer.dynamic
        || buffer.dynamic > FOOTPRINTS
    {
        return Ok(false);
    }
    let mut rects = Vec::new();
    bytes.reserve_capacity(&mut rects, footprints.len())?;
    rects.extend(footprints.iter().copied().filter(|rect| !rect.is_empty()));
    let spend = |visited: usize| match comparisons.checked_sub(visited) {
        Some(left) => {
            *comparisons = left;
            true
        }
        None => false,
    };
    let admitted = disjoint(&mut rects, spend, bytes)?;
    let released = rects.capacity() * size_of::<DeviceRect>();
    drop(rects);
    bytes.refund(released);
    Ok(admitted)
}

/// Whether no two rectangles share a pixel; false once `spend` refuses the
/// rectangles a sweep step visits.
fn disjoint(
    rects: &mut [DeviceRect],
    mut spend: impl FnMut(usize) -> bool,
    bytes: &mut Meter,
) -> Result<bool> {
    rects.sort_unstable_by_key(|r| r.x0);
    let mut active: Vec<DeviceRect> = Vec::new();
    let mut disjoint = true;
    for rect in rects.iter() {
        if !spend(active.len()) {
            disjoint = false;
            break;
        }
        active.retain(|other| other.x1 > rect.x0);
        if active
            .iter()
            .any(|other| other.y0 < rect.y1 && rect.y0 < other.y1)
        {
            disjoint = false;
            break;
        }
        bytes.reserve_capacity(&mut active, 1)?;
        active.push(*rect);
    }
    let released = active.capacity() * size_of::<DeviceRect>();
    drop(active);
    bytes.refund(released);
    Ok(disjoint)
}

impl Survey<'_> {
    /// Chooses formats from the innermost buffer outwards and records each
    /// buffer's error bound in levels.
    pub(super) fn resolve(
        &self,
        node: usize,
        resolved: &mut Vec<Option<(Format, f64)>>,
        depth: usize,
        bytes: &mut Meter,
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
            let (_, bound) = self.resolve(child, resolved, depth + 1, bytes)?;
            injected += (bound * (1.0 + 256.0 / 255.0) + F64_STORE) * count as f64;
        }
        for &(mask, count) in &buffer.masks {
            let (_, bound) = self.resolve(mask, resolved, depth + 1, bytes)?;
            injected += 256.0 / 255.0 * bound * count as f64;
        }
        let stores = buffer.stores as f64;
        let wide = (stores * F64_STORE + MARGIN + injected, Format::F64);
        let n16 = !buffer.forced && buffer.page && n16_admits(buffer, resolved, bytes)?;
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
    pub(super) fn widen(
        &self,
        root: usize,
        resolved: &mut [Option<(Format, f64)>],
        bytes: &mut Meter,
    ) -> Result<()> {
        // 1: visited while narrow, 2: visited while F64.
        let mut seen = Vec::new();
        bytes.reserve_capacity(&mut seen, self.buffers.len())?;
        seen.resize(self.buffers.len(), 0_u8);
        let mut pending = Vec::new();
        bytes.reserve_capacity(&mut pending, 1)?;
        pending.push((root, false));
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
                bytes.reserve_capacity(&mut pending, 1)?;
                pending.push((child, wide));
            }
        }
        let released =
            seen.capacity() * size_of::<u8>() + pending.capacity() * size_of::<(usize, bool)>();
        drop(seen);
        drop(pending);
        bytes.refund(released);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::Layout;
    use crate::prepare::storage::{Dynamic, Inputs};
    use crate::testing::Build;
    use pagura_color::Process;
    use pagura_core::hash::FastMap;

    #[test]
    fn failed_and_forced_proofs_need_no_n16_scratch() {
        let build = Build::new(4.0, 4.0);
        let mut survey = Survey {
            scene: &build.scene,
            inputs: Inputs {
                page: Layout {
                    process: Process::Rgb,
                    spots: 0,
                },
                simulate: false,
                policy: false,
                depth: 64,
                transform: build.transform(),
                pixels: build.page(),
                user_unit: 1.0,
            },
            buffers: Vec::new(),
            groups: FastMap::default(),
            masks: FastMap::default(),
            cells: FastMap::default(),
            open: Vec::new(),
            outlines: FastMap::default(),
            transfers: false,
            comparisons: APART_COMPARISONS,
        };
        let mut fixture = Meter::new(1 << 20, "proof fixture");
        let node = survey
            .buffer(survey.inputs.page, false, &mut fixture)
            .unwrap();
        let buffer = &mut survey.buffers[node];
        buffer.page = true;
        buffer.stores = 1;
        buffer.dynamic = 1;
        fixture.reserve_capacity(&mut buffer.dynamics, 1).unwrap();
        buffer.dynamics.push(Dynamic {
            rect: DeviceRect {
                x0: 0,
                y0: 0,
                x1: 2,
                y1: 2,
            },
            eta: 1.0,
            child: None,
            mask: None,
        });
        let mut no_scratch = Meter::new(0, "no proof scratch");
        assert!(!n16_admits(buffer, &[None], &mut no_scratch).unwrap());
        buffer.forced = true;
        buffer.dynamics[0].eta = 0.0;
        let mut resolved = vec![None];
        assert_eq!(
            survey
                .resolve(node, &mut resolved, 0, &mut no_scratch)
                .unwrap()
                .0,
            Format::F64
        );
        assert_eq!(no_scratch.used(), 0);
    }
}
