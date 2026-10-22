//! Glyph runs (ADR 0004 §4). A placement's span goes into one gathered
//! band row unless it meets pixels another placement's bounds also hold;
//! such spans store at once, placement by placement in content order. After
//! the last placement every gathered row stores in one span. Each pixel
//! thus receives the stores it would receive with one item per placement,
//! in the same order: one gathered store where only one placement can
//! cover it, and a zero-coverage store, which leaves every format as it is,
//! at the gaps of a gathered row. Results are byte-identical.

use super::{Cover, Executor, Frame, Tops, clamp};
use crate::plan::{Glyph, Item, Run, intersect};
use pagura_core::convert::u32_to_usize;
use pagura_raster::DeviceRect;

/// An empty gathered row.
pub(crate) const EMPTY: [u32; 2] = [u32::MAX, 0];

impl Executor<'_> {
    pub(super) fn glyph_run(&mut self, item: &Item, run: &Run, frame: Frame, tops: Tops) {
        let mark = &run.mark;
        if let Some(mask) = mark.compositing.mask {
            self.ensure_mask(mask, tops);
        }
        self.count_shape(item);
        let opaque = self.opaque(mark, frame);
        let rect = intersect(item.bounds, frame.rect);
        let gather = opaque.is_none() && run.gather && !rect.is_empty();
        let mut coverage = std::mem::take(&mut self.ws.coverage);
        for glyph in &run.glyphs {
            if glyph.bands[0] > self.band || glyph.bands[1] < self.band {
                continue;
            }
            let within = intersect(glyph.bounds, frame.rect);
            if within.is_empty() {
                continue;
            }
            self.producer
                .rasterize(Some(glyph.shape), mark.cover.clip, self.band, &mut coverage);
            if let Some(opaque) = opaque {
                self.opaque_store(&coverage, frame, opaque);
                continue;
            }
            // A gathered span stays within its placement's bounds, as the
            // shared rectangles assume.
            let rows = if gather { within } else { frame.rect };
            for row in coverage.rows() {
                if row.y < rows.y0 || row.y >= rows.y1 {
                    continue;
                }
                for span in row.spans() {
                    if gather {
                        if let Some((x, cover)) = clamp(span, within) {
                            self.gather_span(run, glyph, (rect, frame), (x, row.y), cover);
                        }
                    } else if let Some((x, cover)) = clamp(span, frame.rect) {
                        self.mark_span(mark, frame, (x, row.y), cover);
                    }
                }
            }
        }
        self.ws.coverage = coverage;
        if gather {
            self.store_gathered(run, frame, rect);
        }
    }

    /// One span of `glyph`: stored now when it covers a pixel another
    /// placement's bounds also hold, gathered otherwise. Storing a
    /// placement's own pixels at once is always exact; a shared pixel may
    /// gather only at coverage 0, which stores nothing and never overwrites
    /// another placement's coverage there, as that is 0 too or stored now.
    fn gather_span(
        &mut self,
        run: &Run,
        glyph: &Glyph,
        (rect, frame): (DeviceRect, Frame),
        (x, y): (u32, u32),
        cover: Cover<'_>,
    ) {
        let end = x.saturating_add(u32::try_from(cover.len()).unwrap_or(0));
        let covers = |r: &DeviceRect| {
            let (start, stop) = (r.x0.max(x), r.x1.min(end));
            match cover.slice(u32_to_usize(start - x), u32_to_usize(stop - x)) {
                Cover::Full(len) => len > 0,
                Cover::Partial(coverage) => coverage.iter().any(|&c| c != 0),
            }
        };
        let shared = run
            .shared(glyph)
            .iter()
            .filter(|r| r.y0 <= y && y < r.y1 && r.x0 < end && x < r.x1)
            .any(covers);
        if shared {
            self.mark_span(&run.mark, frame, (x, y), cover);
        } else {
            self.write_gathered(rect, (x, y), cover);
        }
    }

    /// Copies `cover` into the gathered row of `y` from column `x`.
    fn write_gathered(&mut self, rect: DeviceRect, (x, y): (u32, u32), cover: Cover<'_>) {
        let width = u32_to_usize(rect.width());
        let row = u32_to_usize(y - rect.y0);
        let column = u32_to_usize(x - rect.x0);
        let (rows, extents) = &mut self.ws.gathered;
        let start = row * width + column;
        let Some(out) = rows.get_mut(start..start + cover.len()) else {
            return;
        };
        match cover {
            Cover::Full(_) => out.fill(255),
            Cover::Partial(coverage) => out.copy_from_slice(coverage),
        }
        if let Some(extent) = extents.get_mut(row) {
            let end = x.saturating_add(u32::try_from(cover.len()).unwrap_or(0));
            *extent = [extent[0].min(x), extent[1].max(end)];
        }
    }

    /// Stores every gathered row and clears it.
    fn store_gathered(&mut self, run: &Run, frame: Frame, rect: DeviceRect) {
        let width = u32_to_usize(rect.width());
        let (mut rows, mut extents) = std::mem::take(&mut self.ws.gathered);
        for (row, y) in (rect.y0..rect.y1).enumerate() {
            let Some(extent) = extents.get_mut(row) else {
                break;
            };
            let [lo, hi] = std::mem::replace(extent, EMPTY);
            if lo >= hi {
                continue;
            }
            let start = row * width + u32_to_usize(lo - rect.x0);
            let end = row * width + u32_to_usize(hi - rect.x0);
            if let Some(coverage) = rows.get(start..end) {
                self.mark_span(&run.mark, frame, (lo, y), Cover::Partial(coverage));
            }
            if let Some(coverage) = rows.get_mut(start..end) {
                coverage.fill(0);
            }
        }
        self.ws.gathered = (rows, extents);
    }
}
