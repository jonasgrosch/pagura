//! Reusable working storage for a sequence of independent path fills.

use super::{FillRule, Mask};
use crate::coverage::{Geometry, Scratch, Shape, capacity_bytes};
use crate::{GlyphGeometry, IntRect, Paint, Path, Pixmap, Point, Transform};
use pagura_core::{Error, Limits, Result};
use std::sync::Arc;

/// Owns temporary raster storage for one rendering context, without caching
/// geometry or quantizing positions. Independent fills retain their own masks;
/// return a consumed mask with [`Self::recycle`] to reuse its allocation too.
///
/// Retained capacities are bounded by [`Self::set_retained_limit`], which is
/// zero for a new rasterizer. Temporary geometry keeps the per-fill segment
/// and work budgets. Nothing is shared between rendering contexts or threads.
#[derive(Debug, Default)]
pub struct Rasterizer {
    scratch: Scratch,
    scratch_bytes: usize,
    coverage: Vec<u8>,
    shapes: Vec<Shape>,
    maximum: usize,
}

impl Rasterizer {
    /// Estimated bytes retained in working buffers, including vector capacities.
    /// Returned masks and their shared paths belong to the caller and are excluded.
    pub fn retained_bytes(&self) -> usize {
        self.scratch_bytes
            .saturating_add(self.coverage.capacity())
            .saturating_add(capacity_bytes(&self.shapes))
    }

    /// Bounds the bytes kept in working buffers between fills, releasing
    /// them now if they exceed `bytes`. The caller owns this allowance: a
    /// renderer passes what its own memory accounting leaves free.
    pub fn set_retained_limit(&mut self, bytes: usize) {
        self.maximum = bytes;
        self.trim();
    }

    /// Releases all working buffers, keeping the retained limit. It does not
    /// affect previously returned masks.
    pub fn clear(&mut self) {
        *self = Self {
            maximum: self.maximum,
            ..Self::default()
        };
    }

    /// Returns a consumed mask's storage. Geometry is released immediately;
    /// recycling never retains its paths or changes a later fill's result.
    /// At most one coverage buffer is kept, subject to the retained byte budget.
    pub fn recycle(&mut self, mut mask: Mask) {
        mask.shapes.clear();
        self.coverage = mask.coverage;
        self.shapes = mask.shapes;
        self.trim();
    }

    fn trim(&mut self) {
        if self.retained_bytes() > self.maximum {
            self.clear();
        }
    }

    /// Fills a path clipped geometrically to `clip`, restricted to its device
    /// bounds (8.5.3.3). A shared path is not copied. Global coordinates and
    /// all coverage arithmetic match [`Mask::clipped_path`].
    ///
    /// # Errors
    /// Returns invalid geometry and caller-budget errors as
    /// [`Mask::from_path`]. No partial mask is returned on failure;
    /// the rasterizer remains usable after an error.
    pub fn clipped_path(
        &mut self,
        clip: &Mask,
        path: impl Into<Arc<Path>>,
        transform: Transform,
        rule: FillRule,
        limits: Limits,
    ) -> Result<Option<Mask>> {
        self.trim();
        let result = self.fill(clip, path.into(), transform, rule, limits);
        self.complete(result)
    }

    /// Rasterizes canonical glyph contours at their exact device origin.
    /// Point classification survives flattening; no coverage is cached.
    /// Returned masks retain shared geometry for later geometric intersection.
    ///
    /// # Errors
    /// Returns geometry, work, source-count and allocation limit errors.
    pub fn clipped_glyph(
        &mut self,
        clip: &Mask,
        glyph: Arc<GlyphGeometry>,
        origin: Point,
        limits: Limits,
    ) -> Result<Option<Mask>> {
        self.trim();
        let result = self.place(clip, glyph, origin, limits);
        self.complete(result)
    }

    fn complete(&mut self, result: Result<Option<Mask>>) -> Result<Option<Mask>> {
        self.shapes.clear();
        if result.is_err() {
            self.clear();
        } else {
            self.trim();
        }
        result
    }

    fn fill(
        &mut self,
        clip: &Mask,
        path: Arc<Path>,
        transform: Transform,
        rule: FillRule,
        limits: Limits,
    ) -> Result<Option<Mask>> {
        // Only retained fill/clip certificates opt into boundary scan conversion.
        // Ordinary paths and stroke outlines take the original raster path.
        let (path, transform, rule) =
            match crate::empty_fill::outline(&path, transform, rule, &limits)? {
                Some(outline) => (Arc::new(outline), Transform::IDENTITY, FillRule::NonZero),
                None => (path, transform, rule),
            };
        let Some(area) = crate::bounds::region(&path, transform, clip.area, &limits)? else {
            return Ok(None);
        };
        let shape = Shape {
            geometry: Geometry::Path(path),
            transform,
            rule,
        };
        self.fill_shape(shape, area, limits, clip)
    }

    fn place(
        &mut self,
        clip: &Mask,
        glyph: Arc<GlyphGeometry>,
        origin: Point,
        limits: Limits,
    ) -> Result<Option<Mask>> {
        crate::flatten::bounded(origin)?;
        let Some(bounds) = glyph.bounds() else {
            return Ok(None);
        };
        let transform = Transform::translate(origin.x, origin.y);
        let Some(area) = crate::bounds::hull_region(bounds.map(|p| p * transform), clip.area)?
        else {
            return Ok(None);
        };
        let shape = Shape {
            geometry: Geometry::Glyph(glyph),
            transform,
            rule: FillRule::NonZero,
        };
        self.fill_shape(shape, area, limits, clip)
    }

    fn fill_shape(
        &mut self,
        shape: Shape,
        area: IntRect,
        limits: Limits,
        clip: &Mask,
    ) -> Result<Option<Mask>> {
        self.shapes.clear();
        if limits.mask_sources == 0 {
            return Err(Error::limit("mask source budget exceeded"));
        }
        self.shapes.push(shape);
        for shape in &clip.shapes {
            if !shape.covers(area) {
                if self.shapes.len() >= limits.mask_sources {
                    return Err(Error::limit("mask source budget exceeded"));
                }
                self.shapes.push(shape.clone());
            }
        }
        let count = super::pixel_budget(area, &limits)?;
        self.coverage
            .try_reserve_exact(count.saturating_sub(self.coverage.len()))
            .map_err(|_| Error::limit("mask allocation failed"))?;
        self.coverage.resize(count, 0);
        crate::coverage::coverage(
            area,
            &self.shapes,
            &mut self.coverage,
            &limits,
            &mut self.scratch,
        )?;
        self.scratch_bytes = self.scratch.retained_bytes();
        Ok(Some(Mask {
            area,
            limits,
            coverage: std::mem::take(&mut self.coverage),
            shapes: std::mem::take(&mut self.shapes),
        }))
    }

    /// Fills a path into `target`, whose pixels are those of `clip`, and
    /// immediately recycles the temporary mask.
    ///
    /// # Errors
    /// As [`Self::clipped_path`]; rejects a clip whose size differs from the target.
    pub fn fill_path(
        &mut self,
        target: &mut Pixmap,
        path: impl Into<Arc<Path>>,
        transform: Transform,
        rule: FillRule,
        paint: &Paint,
        clip: &Mask,
    ) -> Result<()> {
        super::check_clip(target, clip)?;
        if let Some(mask) = self.clipped_path(clip, path, transform, rule, *target.limits())? {
            self.paint(target, mask, *paint, clip);
        }
        Ok(())
    }

    /// Fills one canonical glyph into `target`, whose pixels are those of
    /// `clip`, and recycles its mask storage.
    ///
    /// # Errors
    /// As [`Self::clipped_glyph`]; rejects a clip whose size differs from the target.
    pub fn fill_glyph(
        &mut self,
        target: &mut Pixmap,
        glyph: Arc<GlyphGeometry>,
        origin: Point,
        paint: &Paint,
        clip: &Mask,
    ) -> Result<()> {
        super::check_clip(target, clip)?;
        if let Some(mask) = self.clipped_glyph(clip, glyph, origin, *target.limits())? {
            self.paint(target, mask, *paint, clip);
        }
        Ok(())
    }

    /// Composites a mask clipped to `clip` into `target` and recycles it.
    fn paint(&mut self, target: &mut Pixmap, mask: Mask, paint: Paint, clip: &Mask) {
        // A clipped mask lies within its clip, so the offsets cannot underflow.
        let offset = (mask.area.x() - clip.area.x(), mask.area.y() - clip.area.y());
        super::composite(target, &mask, paint, offset);
        self.recycle(mask);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{IntRect, PathBuilder};

    fn rectangle(x: f64, y: f64, width: f64, height: f64) -> Arc<Path> {
        let mut path = PathBuilder::new();
        path.rect(x, y, width, height);
        Arc::new(path.finish())
    }

    #[test]
    fn recycled_masks_intersect_geometry_and_release_paths() {
        let limits = Limits::default();
        let clip = Mask::from_path(
            IntRect::new(10, 20, 2, 2).unwrap(),
            rectangle(10.25, 20.25, 1.5, 1.5),
            Transform::IDENTITY,
            FillRule::NonZero,
            Limits {
                pixels: 4,
                ..limits
            },
        )
        .unwrap();
        let path = rectangle(10.75, 20.0, 1.0, 2.0);
        let mut raster = Rasterizer::default();
        raster.set_retained_limit(1 << 20);
        for _ in 0..3 {
            let mask = raster
                .clipped_path(
                    &clip,
                    Arc::clone(&path),
                    Transform::IDENTITY,
                    FillRule::NonZero,
                    limits,
                )
                .unwrap()
                .unwrap();
            // Rectangle intersection areas: 3/16 and 9/16 in each row.
            assert_eq!(mask.area(), IntRect::new(10, 20, 2, 2).unwrap());
            assert_eq!(mask.coverage(), [48, 143, 48, 143]);
            assert_eq!(Arc::strong_count(&path), 2);
            raster.recycle(mask);
            assert_eq!(Arc::strong_count(&path), 1);
            assert!(raster.retained_bytes() > 0);
            assert!(raster.retained_bytes() <= 1 << 20);
        }
    }

    #[test]
    fn errors_and_smaller_budgets_do_not_leak_sweep_state() {
        let clip = Mask::from_path(
            IntRect::new(0, 0, 2, 2).unwrap(),
            rectangle(0.0, 0.0, 2.0, 2.0),
            Transform::IDENTITY,
            FillRule::NonZero,
            Limits {
                pixels: 4,
                ..Limits::default()
            },
        )
        .unwrap();
        let path = rectangle(0.25, 0.25, 0.5, 0.5);
        let mut raster = Rasterizer::default();
        let exhausted = Limits {
            raster_work: 0,
            ..Limits::default()
        };
        assert!(
            raster
                .clipped_path(
                    &clip,
                    Arc::clone(&path),
                    Transform::IDENTITY,
                    FillRule::NonZero,
                    exhausted
                )
                .is_err()
        );
        assert_eq!(Arc::strong_count(&path), 1);
        raster.set_retained_limit(0);
        let mask = raster
            .clipped_path(
                &clip,
                path,
                Transform::IDENTITY,
                FillRule::EvenOdd,
                Limits::default(),
            )
            .unwrap()
            .unwrap();
        assert_eq!(mask.coverage(), [64]); // 1/4 pixel, independently rounded.
        assert_eq!(raster.retained_bytes(), 0);
        raster.recycle(mask);
        assert_eq!(raster.retained_bytes(), 0);
        let full = raster
            .clipped_path(
                &clip,
                rectangle(0.0, 0.0, 2.0, 2.0),
                Transform::IDENTITY,
                FillRule::NonZero,
                Limits::default(),
            )
            .unwrap()
            .unwrap();
        assert_eq!(full.coverage(), [255; 4]);
    }
}
