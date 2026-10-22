//! Glyph runs (ADR 0004 §4). Placements whose bounds meet no other
//! placement's are rasterized together: the producer merges their rows,
//! each pixel holding the coverage of the one placement that can cover it
//! and zero in the gaps, and each merged span stores at once. The other
//! placements store one by one in content order. Every pixel thus receives
//! the stores it would receive with one item per placement, in the same
//! order, plus zero-coverage stores at the gaps, which leave every format
//! as it is. Results are byte-identical.
//!
//! Under an opaque store, a binned placement whose clip state keeps all of
//! it stores its mask's rows straight into the page, in content order with
//! the placements that store one by one.

use super::{Cover, Executor, Frame, Opaque, Runs, Tops, clamp, opaque_store};
use crate::plan::{Glyph, Item, Mark, Run, intersect};
use pagura_raster::{BandCoverage, DeviceRect, GlyphMask};
use std::ops::Range;

impl Executor<'_> {
    pub(super) fn glyph_run(&mut self, item: &Item, run: &Run, frame: Frame, tops: Tops) {
        let mark = &run.mark;
        if let Some(mask) = mark.compositing.mask {
            self.ensure_mask(mask, tops);
        }
        self.count_shape(item);
        let opaque = self.opaque(mark, frame);
        let mut coverage = std::mem::take(&mut self.ws.coverage);
        let mut alone = std::mem::take(&mut self.ws.run_shapes);
        alone.clear();
        let direct = |glyph: &Glyph| opaque.is_some() && self.producer.mask(glyph.shape).is_some();
        alone.extend(
            run.glyphs
                .iter()
                .filter(|glyph| glyph.alone && self.meets(glyph, frame) && !direct(glyph))
                .map(|glyph| glyph.shape),
        );
        let mut next = 0;
        while let Some(shapes) = alone.get(next..).filter(|rest| !rest.is_empty()) {
            let taken =
                self.producer
                    .rasterize_run(shapes, mark.cover.clip, self.band, &mut coverage);
            next += taken.max(1);
            self.store_coverage(mark, frame, opaque, &coverage);
        }
        for glyph in &run.glyphs {
            if !self.meets(glyph, frame) {
                continue;
            }
            if let Some(opaque) = opaque
                && let Some(mask) = self.producer.mask(glyph.shape)
            {
                let rows = self.rows.clone();
                opaque_store(self.ws, &MaskRuns { mask, rows }, frame, opaque);
                continue;
            }
            if glyph.alone {
                continue;
            }
            self.producer
                .rasterize(Some(glyph.shape), mark.cover.clip, self.band, &mut coverage);
            self.store_coverage(mark, frame, opaque, &coverage);
        }
        self.ws.coverage = coverage;
        self.ws.run_shapes = alone;
    }

    /// Whether `glyph` can cover a pixel of `frame` in this band.
    fn meets(&self, glyph: &Glyph, frame: Frame) -> bool {
        glyph.bands[0] <= self.band
            && self.band <= glyph.bands[1]
            && !intersect(glyph.bounds, frame.rect).is_empty()
    }

    /// Stores every span of `coverage` within `frame`.
    fn store_coverage(
        &mut self,
        mark: &Mark,
        frame: Frame,
        opaque: Option<Opaque>,
        coverage: &BandCoverage,
    ) {
        if let Some(opaque) = opaque {
            self.opaque_store(coverage, frame, opaque);
            return;
        }
        for row in coverage.rows() {
            if row.y < frame.rect.y0 || row.y >= frame.rect.y1 {
                continue;
            }
            for span in row.spans() {
                if let Some((x, cover)) = clamp(span, frame.rect) {
                    self.mark_span(mark, frame, (x, row.y), cover);
                }
            }
        }
    }
}

/// The rows of a glyph mask within a band.
struct MaskRuns<'a> {
    mask: &'a GlyphMask,
    rows: Range<u32>,
}

impl Runs for MaskRuns<'_> {
    fn runs(&self, rect: DeviceRect, mut run: impl FnMut(u32, u32, Cover<'_>)) {
        let bounds = self.mask.bounds();
        let start = self.rows.start.max(rect.y0).max(bounds.y0);
        let end = self.rows.end.min(rect.y1).min(bounds.y1);
        for y in start..end {
            let Some((x, values)) = self.mask.row(y) else {
                continue;
            };
            let lo = x.max(rect.x0);
            let hi = x
                .saturating_add(u32::try_from(values.len()).unwrap_or(u32::MAX))
                .min(rect.x1);
            if lo < hi
                && let Some(values) = values.get((lo - x) as usize..(hi - x) as usize)
            {
                run(y, lo, Cover::Partial(values));
            }
        }
    }
}
