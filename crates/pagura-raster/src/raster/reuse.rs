//! Reusable working storage for a sequence of independent path fills.

use super::{FillRule, Mask};
use crate::coverage::{Scratch, Shape};
use crate::{Paint, Path, Pixmap, Transform};
use pagura_core::{Error, Limits, Result};
use std::sync::Arc;

// Borrowed callers acquire ownership only after bounds and budget validation.
pub(super) enum PathSource<'a> {
    Borrowed(&'a Path),
    Shared(Arc<Path>),
}
impl PathSource<'_> {
    fn path(&self) -> &Path {
        match self {
            Self::Borrowed(path) => path,
            Self::Shared(path) => path,
        }
    }
    fn into_shared(self) -> Arc<Path> {
        match self {
            Self::Borrowed(path) => Arc::new(path.clone()),
            Self::Shared(path) => path,
        }
    }
}

/// Owns temporary raster storage for one rendering context, without caching
/// geometry or quantizing positions. Independent fills retain their own masks;
/// return a consumed mask with [`Self::recycle`] to reuse its allocation too.
///
/// Retained capacities are bounded by the most recent call's
/// [`Limits::decoded_bytes`]. Temporary geometry keeps the per-fill segment and
/// work budgets. Nothing is shared between rendering contexts or threads.
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
            .saturating_add(self.shapes.capacity().saturating_mul(size_of::<Shape>()))
    }

    /// Releases all working buffers. It does not affect previously returned masks.
    pub fn clear(&mut self) {
        *self = Self::default();
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
            let maximum = self.maximum;
            self.clear();
            self.maximum = maximum;
        }
    }

    /// Fills a path clipped geometrically to `clip`, restricted to its device
    /// bounds (8.5.3.3). Shared path ownership avoids copying path segments.
    /// Global coordinates and all coverage arithmetic match [`Mask::clipped_path`].
    ///
    /// # Errors
    /// Returns invalid geometry and caller-budget errors as
    /// [`Mask::from_path_with_limits`]. No partial mask is returned on failure;
    /// the rasterizer remains usable after an error.
    pub fn clipped_path(
        &mut self,
        clip: &Mask,
        path: Arc<Path>,
        transform: Transform,
        rule: FillRule,
        limits: Limits,
    ) -> Result<Option<Mask>> {
        self.path_mask(
            PathSource::Shared(path),
            transform,
            rule,
            limits,
            ((clip.width, clip.height), clip.origin, Some(clip)),
        )
    }

    pub(super) fn path_mask(
        &mut self,
        path: PathSource<'_>,
        transform: Transform,
        rule: FillRule,
        limits: Limits,
        viewport: ((u32, u32), (u32, u32), Option<&Mask>),
    ) -> Result<Option<Mask>> {
        self.maximum = limits.decoded_bytes;
        self.trim();
        let result = self.fill(path, transform, rule, limits, viewport);
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
        path: PathSource<'_>,
        transform: Transform,
        rule: FillRule,
        limits: Limits,
        (size, origin, clip): ((u32, u32), (u32, u32), Option<&Mask>),
    ) -> Result<Option<Mask>> {
        let Some((x, y, width, height)) =
            crate::bounds::region(path.path(), transform, size, origin, &limits)?
        else {
            return Ok(None);
        };
        self.shapes.clear();
        if limits.mask_sources == 0 {
            return Err(Error::limit("mask source budget exceeded"));
        }
        self.shapes.push(Shape {
            path: path.into_shared(),
            transform,
            rule,
        });
        for shape in clip.into_iter().flat_map(|clip| &clip.shapes) {
            if !shape.covers((x, y), (width, height)) {
                if self.shapes.len() >= limits.mask_sources {
                    return Err(Error::limit("mask source budget exceeded"));
                }
                self.shapes.push(shape.clone());
            }
        }
        let count = usize::try_from(u64::from(width) * u64::from(height))
            .ok()
            .filter(|&n| n <= limits.pixels)
            .ok_or(Error::limit("mask pixel budget exceeded"))?;
        self.coverage
            .try_reserve_exact(count.saturating_sub(self.coverage.len()))
            .map_err(|_| Error::limit("mask allocation failed"))?;
        self.coverage.resize(count, 0);
        crate::coverage::coverage(
            width,
            height,
            &self.shapes,
            &mut self.coverage,
            (x, y),
            &limits,
            &mut self.scratch,
        )?;
        self.scratch_bytes = self.scratch.retained_bytes();
        Ok(Some(Mask {
            width,
            height,
            origin: (x, y),
            limits,
            coverage: std::mem::take(&mut self.coverage),
            shapes: std::mem::take(&mut self.shapes),
        }))
    }

    /// Fills a tile and immediately recycles the temporary mask.
    ///
    /// # Errors
    /// As [`Self::clipped_path`]; rejects a clip whose size differs from the tile.
    pub fn fill_path(
        &mut self,
        target: &mut Pixmap,
        path: Arc<Path>,
        transform: Transform,
        rule: FillRule,
        paint: &Paint,
        clip: &Mask,
    ) -> Result<()> {
        super::check_clip(target, Some(clip))?;
        let origin = clip.origin;
        if let Some(mask) = self.clipped_path(clip, path, transform, rule, *target.limits())? {
            let offset = (
                mask.origin.0.checked_sub(origin.0),
                mask.origin.1.checked_sub(origin.1),
            );
            let result = match offset {
                (Some(x), Some(y)) => super::composite(target, &mask, *paint, (x, y)),
                _ => Err(Error::invalid_argument("mask precedes tile origin")),
            };
            self.recycle(mask);
            result?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PathBuilder;

    fn rectangle(x: f64, y: f64, width: f64, height: f64) -> Arc<Path> {
        let mut path = PathBuilder::new();
        path.rect(x, y, width, height);
        Arc::new(path.finish())
    }

    #[test]
    fn recycled_masks_intersect_geometry_and_release_paths() {
        let limits = Limits::default();
        let clip = Mask::from_path_at(
            2,
            2,
            &rectangle(10.25, 20.25, 1.5, 1.5),
            Transform::IDENTITY,
            FillRule::NonZero,
            4,
            (10, 20),
        )
        .unwrap();
        let path = rectangle(10.75, 20.0, 1.0, 2.0);
        let mut raster = Rasterizer::default();
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
            assert_eq!(mask.origin(), (10, 20));
            assert_eq!(mask.coverage(), [48, 143, 48, 143]);
            assert_eq!(Arc::strong_count(&path), 2);
            raster.recycle(mask);
            assert_eq!(Arc::strong_count(&path), 1);
            assert!(raster.retained_bytes() > 0);
            assert!(raster.retained_bytes() <= limits.decoded_bytes);
        }
    }

    #[test]
    fn errors_and_smaller_budgets_do_not_leak_sweep_state() {
        let clip = Mask::from_path(
            2,
            2,
            &rectangle(0.0, 0.0, 2.0, 2.0),
            Transform::IDENTITY,
            FillRule::NonZero,
            4,
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
        let uncached = Limits {
            decoded_bytes: 0,
            ..Limits::default()
        };
        let mask = raster
            .clipped_path(
                &clip,
                path,
                Transform::IDENTITY,
                FillRule::EvenOdd,
                uncached,
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
