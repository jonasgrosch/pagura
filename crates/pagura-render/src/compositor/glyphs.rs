//! Glyph runs: one cached geometry per outline, placed per glyph (9.3.8).

use super::{Backdrop, Context, Model, Surface, path_active, regions};
use crate::Tile;
use pagura_core::{Error, Limits, Result};
use std::sync::Arc;

impl Context {
    #[expect(clippy::too_many_arguments, reason = "explicit mark and group state")]
    pub(super) fn glyphs(
        &mut self,
        run: &crate::display::GlyphRun,
        tile: Tile,
        target: &mut Surface,
        initial: Backdrop<'_>,
        model: Model,
        knockout: bool,
        depth: usize,
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
            let result = (|| {
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
                        decoded_bytes: self.limits.decoded_bytes.saturating_sub(self.allocated),
                        ..self.limits
                    },
                    &mut self.raster,
                )?;
                // Every placement scans its flattened outline, cached or not.
                self.charge(geometry.vertices())?;
                let bytes = geometry.retained_bytes();
                self.trim_raster(bytes);
                self.allocated = self
                    .allocated
                    .checked_add(bytes)
                    .filter(|&n| n <= self.limits.decoded_bytes)
                    .ok_or(Error::limit("glyph geometry memory exceeded"))?;
                let clip = if let Some(clip) = &clip {
                    Arc::clone(clip)
                } else {
                    let value = self.clip(run.paint.clip.as_ref(), tile)?;
                    clip = Some(Arc::clone(&value));
                    value
                };
                let limits = Limits {
                    decoded_bytes: self.limits.decoded_bytes.saturating_sub(self.allocated),
                    ..self.limits
                };
                let Some(mask) = self.raster.clipped_glyph(
                    &clip,
                    geometry,
                    placement.origin * self.device,
                    limits,
                )?
                else {
                    self.allocated -= bytes;
                    return Ok(());
                };
                let Some(mask) = self.pattern_mask(&run.paint, mask)? else {
                    self.allocated -= bytes;
                    return Ok(());
                };
                let result =
                    self.paint_path((&run.paint, &mask), target, initial, model, knockout, depth);
                self.raster.recycle(mask);
                self.allocated -= bytes;
                self.trim_raster(0);
                result
            })();
            result.map_err(|e| crate::display::contextual(e, position))?;
        }
        Ok(())
    }
}
