//! Soft masks (11.6.5): a mask's group runs for the band before the items
//! it masks, over the pixels they use, and becomes alpha or luminosity
//! values through `TR`; outside its group's content the mask has one value.

use super::{Backdrop, Buffer, Executor, Frame, MaskCache, Read, Tops};
use crate::plan::{Curve, Layout, MaskPlan, Plan, Start, area, intersect};
use crate::process;
use pagura_core::convert::u32_to_usize;
use pagura_interpret::MaskKind;
use pagura_raster::DeviceRect;

impl Executor<'_> {
    /// Computes mask `m`'s values for this band unless they are current;
    /// its group's buffer goes above `tops`.
    pub(super) fn ensure_mask(&mut self, m: u32, tops: Tops) {
        let plan = self.plan;
        let index = u32_to_usize(m);
        let Some(mask) = plan.masks.get(index) else {
            return;
        };
        if self
            .ws
            .masks
            .get(index)
            .is_none_or(|cache| cache.band == Some(self.band))
        {
            return;
        }
        let band = DeviceRect {
            x0: plan.area.region.x0,
            y0: self.rows.start,
            x1: plan.area.region.x1,
            y1: self.rows.end,
        };
        let rect = intersect(mask.extent, band);
        let mut computed = DeviceRect::default();
        if let (false, Some(group)) = (rect.is_empty(), plan.groups.get(u32_to_usize(mask.group))) {
            let layout = group.layout;
            let (buffer, above) = tops.place(group.format, rect, layout);
            if !above.within(plan.stack) {
                debug_assert!(false, "the stacks hold every mask group buffer");
                return;
            }
            let initial = match group.start {
                Start::Opaque(color) => Backdrop::Opaque(color),
                Start::Transparent | Start::Backdrop => Backdrop::Transparent,
            };
            self.start(initial, buffer, rect, layout);
            let frame = Frame {
                group: mask.group,
                buffer,
                rect,
                layout,
                knockout: group.knockout,
                initial,
            };
            self.run(frame, above);
            let ws = &mut *self.ws;
            let read = Read {
                page: &ws.page,
                wide: &ws.wide,
                levels: &ws.levels,
            };
            if let Some(cache) = ws.masks.get_mut(index) {
                let spent = values(mask, (read, buffer, layout), area(rect), cache);
                ws.usage.functions = ws.usage.functions.saturating_add(spent);
                ws.usage.work = ws.usage.work.saturating_add(area(rect));
                computed = rect;
            }
        }
        if let Some(cache) = self.ws.masks.get_mut(index) {
            cache.rect = computed;
            cache.band = Some(self.band);
        }
    }
}

/// Converts `count` pixels of the mask group's buffer into values; returns
/// the function work spent. Equal values share one transfer evaluation.
fn values(
    mask: &MaskPlan,
    (read, buffer, layout): (Read<'_>, Buffer, Layout),
    count: usize,
    cache: &mut MaskCache,
) -> usize {
    let n = layout.channels();
    let (mut last, mut mapped, mut work) = (f64::NAN, 0.0, 0);
    for (index, out) in cache.values.iter_mut().take(count).enumerate() {
        let value = match (mask.kind, buffer) {
            (MaskKind::Alpha, Buffer::Wide(offset)) => read
                .wide
                .get(offset + index * layout.stride() + n + 1)
                .copied()
                .unwrap_or(0.0),
            (MaskKind::Alpha, _) => read.pixel(buffer, index, layout).1,
            (MaskKind::Luminosity, _) => {
                let (colors, alpha) = read.pixel(buffer, index, layout);
                let mut color = [0.0; 4];
                for (i, c) in color.iter_mut().enumerate().take(n) {
                    *c = if mask.isolated {
                        colors[i] + (1.0 - alpha) * mask.backdrop[i]
                    } else {
                        colors[i]
                    };
                }
                process::luminosity(layout.process, &color)
            }
        }
        .clamp(0.0, 1.0);
        if value.to_bits() != last.to_bits() {
            mapped = transfer(mask, value);
            work += mask.transfer.as_ref().map_or(0, Curve::cost);
            last = value;
        }
        *out = mapped;
    }
    work
}

/// `TR` applied to a clipped mask value.
pub(crate) fn transfer(mask: &MaskPlan, value: f64) -> f64 {
    mask.transfer
        .as_ref()
        .map_or(value, |f| f.map(value).clamp(0.0, 1.0))
}

/// The values of mask `m` for pixels `x .. x + len` of row `y`, in `out`.
pub(super) fn row<'m>(
    plan: &Plan,
    masks: &[MaskCache],
    out: &'m mut [f64],
    m: u32,
    (x, y): (u32, u32),
    len: usize,
) -> Option<&'m [f64]> {
    let mask = plan.masks.get(u32_to_usize(m))?;
    let cache = masks.get(u32_to_usize(m))?;
    let out = out.get_mut(..len)?;
    out.fill(mask.outside);
    let r = cache.rect;
    if y >= r.y0 && y < r.y1 {
        let end = x.saturating_add(u32::try_from(len).unwrap_or(u32::MAX));
        let (start, stop) = (x.max(r.x0), end.min(r.x1));
        if start < stop {
            let width = u32_to_usize(r.width());
            let begin = u32_to_usize(y - r.y0) * width + u32_to_usize(start - r.x0);
            let count = u32_to_usize(stop - start);
            if let (Some(target), Some(source)) = (
                out.get_mut(u32_to_usize(start - x)..u32_to_usize(start - x) + count),
                cache.values.get(begin..begin + count),
            ) {
                target.copy_from_slice(source);
            }
        }
    }
    Some(out)
}
