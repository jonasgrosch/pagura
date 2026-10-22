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
//! the placements that store one by one. Zero coverage leaves a pixel as
//! it is and full coverage stores the colour, as band coverage does, so
//! the bytes are again those of one item per placement.

use super::{Executor, Extents, Frame, Opaque, Stores, Tops, clamp, opaque_store};
use crate::plan::{Glyph, Item, Mark, Run, intersect};
use pagura_core::convert::u32_to_usize;
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
                opaque_store(self.ws, &MaskRows { mask, rows }, frame, opaque);
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
struct MaskRows<'a> {
    mask: &'a GlyphMask,
    rows: Range<u32>,
}

impl Stores for MaskRows<'_> {
    fn store_into<T>(
        &self,
        page: &mut [T],
        extents: &mut Extents,
        frame: Frame,
        store: impl Fn(&mut [T], Option<&[u8]>),
    ) -> usize {
        let rect = frame.rect;
        let within = DeviceRect {
            y0: rect.y0.max(self.rows.start),
            y1: rect.y1.min(self.rows.end),
            ..rect
        };
        let Some((block, cells, stride)) = self.mask.block(within) else {
            return 0;
        };
        let width = u32_to_usize(rect.width());
        let (columns, rows) = (u32_to_usize(block.width()), u32_to_usize(block.height()));
        let (lo, hi) = (block.x0 - rect.x0, block.x1 - rect.x0);
        let first = u32_to_usize(block.y0 - rect.y0);
        let mut at = first * width + u32_to_usize(lo);
        for values in cells.chunks(stride.max(1)).take(rows) {
            if let (Some(pixels), Some(values)) =
                (page.get_mut(at..at + columns), values.get(..columns))
            {
                store(pixels, Some(values));
            }
            at += width;
        }
        extents.mark_rows(first..first + rows, lo, hi);
        columns * rows
    }
}
