//! Glyph runs (ADR 0004 §4). A constant colour on a narrow page stores
//! each placement as the rasterizer emits it, in content order.
//! Otherwise placements whose bounds meet no other placement's are
//! rasterized together: the producer merges their rows, each pixel holding
//! the coverage of the one placement that can cover it and zero in the
//! gaps, and each merged span stores at once. The other placements store
//! one by one in content order. Every pixel thus receives the stores it
//! would receive with one item per placement, in the same order, plus
//! zero-coverage stores at the gaps, which leave every format as it is.
//! Results are byte-identical. In all paths, a placement that crosses into
//! the next band keeps its rows there instead of being walked again.

use super::{Direct, Executor, Frame, Tops};
use crate::plan::{Glyph, Item, Run, intersect};

impl Executor<'_> {
    pub(super) fn glyph_run(&mut self, item: &Item, run: &Run, frame: Frame, tops: Tops) {
        let mark = &run.mark;
        if let Some(mask) = mark.compositing.mask {
            self.ensure_mask(mask, tops);
        }
        self.count_shape(item);
        let storage = (
            &mut self.ws.page,
            &mut self.ws.wide[..],
            &mut self.ws.levels[..],
        );
        if let Some(mut direct) = Direct::new(self.plan, storage, mark, frame) {
            for glyph in &run.glyphs {
                if meets(glyph, frame, self.band) {
                    let stored = direct.rasterize(|out| {
                        self.shapes.rasterize_glyph_into(
                            self.rasterizer,
                            glyph.shape,
                            self.band,
                            out,
                        );
                    });
                    self.ws.usage.work = self.ws.usage.work.saturating_add(stored);
                }
            }
            return;
        }
        let mut coverage = std::mem::take(&mut self.ws.coverage);
        let mut alone = std::mem::take(&mut self.ws.run_shapes);
        alone.clear();
        alone.extend(
            run.glyphs
                .iter()
                .filter(|glyph| glyph.alone && meets(glyph, frame, self.band))
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
            self.store_coverage(mark, frame, &coverage);
        }
        for glyph in &run.glyphs {
            if glyph.alone || !meets(glyph, frame, self.band) {
                continue;
            }
            self.shapes
                .rasterize_glyph(self.rasterizer, glyph.shape, self.band, &mut coverage);
            self.store_coverage(mark, frame, &coverage);
        }
        self.ws.coverage = coverage;
        self.ws.run_shapes = alone;
    }
}

/// Whether `glyph` can cover a pixel of `frame` in this band.
fn meets(glyph: &Glyph, frame: Frame, band: u32) -> bool {
    glyph.bands[0] <= band
        && band <= glyph.bands[1]
        && !intersect(glyph.bounds, frame.rect).is_empty()
}
