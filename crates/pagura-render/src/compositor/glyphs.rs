//! Glyph runs: one cached geometry per outline, placed per glyph (9.3.8).

use super::{Context, Frame, Surface, path_active, regions};
use crate::Tile;
use crate::display::Placement;
use pagura_core::{Error, Limits, Result};
use pagura_raster::Mask;
use std::sync::Arc;

impl Context {
    pub(super) fn glyphs(
        &mut self,
        run: &crate::display::GlyphRun,
        tile: Tile,
        target: &mut Surface,
        frame: Frame<'_>,
    ) -> Result<()> {
        if !run.paint.color.paints() || !path_active(&run.paint) {
            return Ok(());
        }
        self.charge(run.glyphs.len())?;
        self.charge(run.bounds_work()?)?;
        if !regions::run_visible(run, self.device, tile) {
            // Preserve the per-instance bounds charges while avoiding their work.
            let remaining = self.limits.raster_work - self.work;
            if run.glyphs.len() > remaining {
                self.work = self.limits.raster_work;
                return Err(crate::display::contextual(
                    Error::limit("glyph bounds work exceeded"),
                    run.position(remaining),
                ));
            }
            self.work += run.glyphs.len();
            return Ok(());
        }
        let mut clip = None;
        let mut positions = run.positions.iter().peekable();
        let mut position = run.paint.position;
        for (index, &placement) in run.glyphs.iter().enumerate() {
            while let Some(&(_, p)) = positions.next_if(|(i, _)| *i <= index) {
                position = p;
            }
            self.glyph(run, placement, tile, target, frame, &mut clip)
                .map_err(|e| crate::display::contextual(e, position))?;
        }
        Ok(())
    }
    /// Paints one placement of `run`, culled before its outline is flattened;
    /// `clip` caches the run's clip mask across placements.
    fn glyph(
        &mut self,
        run: &crate::display::GlyphRun,
        placement: Placement,
        tile: Tile,
        target: &mut Surface,
        frame: Frame<'_>,
        clip: &mut Option<Arc<Mask>>,
    ) -> Result<()> {
        let hull = run
            .source(placement)?
            .device_hull(placement.origin, self.device);
        if !regions::glyph_visible(hull, tile, &mut self.work, self.limits.raster_work)? {
            return Ok(());
        }
        let geometry = self.glyphs.geometry(
            run,
            placement,
            self.device,
            &Limits {
                decoded_bytes: self.memory.remaining(),
                ..self.limits
            },
            &mut self.raster,
        )?;
        // Every placement scans its flattened outline, cached or not.
        self.charge(geometry.vertices())?;
        let bytes = geometry.retained_bytes();
        self.trim_raster(bytes);
        let painted = self.holding(bytes, "glyph geometry memory exceeded", |this| {
            let clip = if let Some(clip) = clip {
                Arc::clone(clip)
            } else {
                let value = this.clip(run.paint.clip.as_ref(), tile)?;
                *clip = Some(Arc::clone(&value));
                value
            };
            let limits = Limits {
                decoded_bytes: this.memory.remaining(),
                ..this.limits
            };
            let Some(mask) = this.raster.clipped_glyph(
                &clip,
                geometry,
                placement.origin * this.device,
                limits,
            )?
            else {
                return Ok(false);
            };
            let Some(mask) = this.pattern_mask(&run.paint, mask)? else {
                return Ok(false);
            };
            let result = this.paint_path(&run.paint, &mask, target, frame);
            this.raster.recycle(mask);
            result.map(|()| true)
        })?;
        if painted {
            self.trim_raster(0);
        }
        Ok(())
    }
}
