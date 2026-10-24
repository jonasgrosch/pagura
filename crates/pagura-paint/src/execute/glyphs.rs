//! Glyph runs (ADR 0004 §4). Placements whose bounds meet no other
//! placement's are rasterized together: the producer merges their rows,
//! each pixel holding the coverage of the one placement that can cover it
//! and zero in the gaps, and each merged span stores at once. The other
//! placements store one by one in content order. Every pixel thus receives
//! the stores it would receive with one item per placement, in the same
//! order, plus zero-coverage stores at the gaps, which leave every format
//! as it is. Results are byte-identical. Either way, a placement that
//! crosses into the next band keeps its rows there instead of being walked
//! again.

use super::{Executor, Frame, Tops};
use crate::plan::{Glyph, Item, Run, intersect};

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
            let taken = self.shapes.rasterize_run(
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
            self.shapes
                .rasterize_glyph(self.rasterizer, glyph.shape, self.band, &mut coverage);
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
}
