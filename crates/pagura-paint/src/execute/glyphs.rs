//! Glyph runs (ADR 0004 §4). Placements whose bounds meet no other
//! placement's are rasterized together: the producer merges their rows,
//! each pixel holding the coverage of the one placement that can cover it
//! and zero in the gaps, and each merged span stores at once. The other
//! placements store one by one in content order. Every pixel thus receives
//! the stores it would receive with one item per placement, in the same
//! order, plus zero-coverage stores at the gaps, which leave every format
//! as it is. Results are byte-identical.

use super::{Executor, Frame, Opaque, Tops, clamp};
use crate::plan::{Glyph, Item, Mark, Run, intersect};
use pagura_raster::BandCoverage;

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
        alone.extend(
            run.glyphs
                .iter()
                .filter(|glyph| glyph.alone && self.meets(glyph, frame))
                .map(|glyph| glyph.shape),
        );
        let mut next = 0;
        while let Some(shapes) = alone.get(next..).filter(|rest| !rest.is_empty()) {
            let taken = self.producer.rasterize_run(
                self.rasterizer,
                shapes,
                mark.cover.clip,
                self.band,
                &mut coverage,
            );
            next += taken.max(1);
            self.store_coverage(mark, frame, opaque, &coverage);
        }
        for glyph in &run.glyphs {
            if glyph.alone || !self.meets(glyph, frame) {
                continue;
            }
            self.producer.rasterize(
                self.rasterizer,
                Some(glyph.shape),
                mark.cover.clip,
                self.band,
                &mut coverage,
            );
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
