//! Reusable working storage for a sequence of independent path fills.

use super::{ClipRegion, Coverage, FillRule, Mask, Rectangle};
use crate::coverage::{Geometry, Scratch, Shape};
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

    /// Returns a consumed mask's or rectangle's storage. Geometry is released
    /// immediately; recycling never retains its paths or changes a later
    /// fill's result. At most one coverage buffer is kept, subject to the
    /// retained byte budget.
    pub fn recycle(&mut self, coverage: impl Into<Coverage>) {
        match coverage.into() {
            Coverage::Mask(Mask {
                mut region,
                coverage,
            }) => {
                region.shapes.clear();
                self.coverage = coverage;
                self.shapes = region.shapes;
            }
            Coverage::Rectangle(rectangle) => self.coverage = rectangle.rows.into_samples(),
        }
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
        clip: &ClipRegion,
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

    /// Fills a path clipped to `clip` as [`Self::clipped_path`] does, but keeps
    /// an axis-aligned rectangle within rectangular clips in closed form: its
    /// runs equal the integrated mask's, and validation and work charges are
    /// unchanged, but no mask is written or scanned. Other fills return a mask.
    ///
    /// # Errors
    /// As [`Self::clipped_path`].
    pub fn clipped_coverage(
        &mut self,
        clip: &ClipRegion,
        path: Arc<Path>,
        transform: Transform,
        rule: FillRule,
        limits: Limits,
    ) -> Result<Option<Coverage>> {
        self.maximum = limits.decoded_bytes;
        self.trim();
        let result = (|| {
            let Some((shape, region)) = shape(
                PathSource::Shared(path),
                transform,
                rule,
                &limits,
                ((clip.width, clip.height), clip.origin),
            )?
            else {
                return Ok(None);
            };
            let count = self.sources(shape, region, limits, Some(clip))?;
            let (x, y, width, height) = region;
            if let Some(rows) = crate::coverage::rectangle(
                (width, height),
                count,
                &self.shapes,
                (x, y),
                &limits,
                &mut self.scratch,
                &mut self.coverage,
            )? {
                return Ok(Some(Coverage::Rectangle(Rectangle {
                    origin: (x, y),
                    area: count,
                    rows,
                })));
            }
            self.integrate(region, count, limits)
                .map(|mask| Some(Coverage::Mask(mask)))
        })();
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
        clip: &ClipRegion,
        glyph: Arc<crate::GlyphGeometry>,
        origin: crate::Point,
        limits: Limits,
    ) -> Result<Option<Mask>> {
        self.maximum = limits.decoded_bytes;
        self.trim();
        let result = (|| {
            crate::flatten::bounded(origin)?;
            // The curve sweep may reach beyond the polygon; the fallback may not.
            let bounds = if glyph.curves().is_some()
                && crate::coverage::quadratic::rectangular_clips(&clip.shapes)
            {
                glyph.bounds()
            } else {
                glyph.polygon_bounds()
            };
            let Some(bounds) = bounds else {
                return Ok(None);
            };
            let transform = Transform::translate(origin.x, origin.y);
            let Some(region) = crate::bounds::hull_region(
                bounds.map(|p| p * transform),
                (clip.width, clip.height),
                clip.origin,
            )?
            else {
                return Ok(None);
            };
            self.fill_shape(
                Shape {
                    geometry: Geometry::Glyph(glyph),
                    transform,
                    rule: FillRule::NonZero,
                },
                region,
                limits,
                Some(clip),
            )
        })();
        self.complete(result)
    }

    fn complete<T>(&mut self, result: Result<T>) -> Result<T> {
        self.shapes.clear();
        if result.is_err() {
            self.clear();
        } else {
            self.trim();
        }
        result
    }

    /// Fills one canonical glyph and recycles its mask storage.
    ///
    /// # Errors
    /// As [`Self::clipped_glyph`]; rejects incompatible target and clip sizes.
    pub fn fill_glyph(
        &mut self,
        target: &mut Pixmap,
        glyph: Arc<crate::GlyphGeometry>,
        origin: crate::Point,
        paint: &Paint,
        clip: &ClipRegion,
    ) -> Result<()> {
        super::check_clip(target, Some(clip))?;
        if let Some(mask) = self.clipped_glyph(clip, glyph, origin, *target.limits())? {
            let offset = (
                mask.origin().0 - clip.origin.0,
                mask.origin().1 - clip.origin.1,
            );
            let result = super::composite(target, &mask, *paint, offset);
            self.recycle(mask);
            result?;
        }
        Ok(())
    }

    pub(super) fn path_mask(
        &mut self,
        path: PathSource<'_>,
        transform: Transform,
        rule: FillRule,
        limits: Limits,
        viewport: ((u32, u32), (u32, u32), Option<&ClipRegion>),
    ) -> Result<Option<Mask>> {
        self.maximum = limits.decoded_bytes;
        self.trim();
        let (size, origin, clip) = viewport;
        let result = (|| {
            let Some((shape, region)) = shape(path, transform, rule, &limits, (size, origin))?
            else {
                return Ok(None);
            };
            self.fill_shape(shape, region, limits, clip)
        })();
        self.complete(result)
    }

    fn fill_shape(
        &mut self,
        shape: Shape,
        region: Region,
        limits: Limits,
        clip: Option<&ClipRegion>,
    ) -> Result<Option<Mask>> {
        let count = self.sources(shape, region, limits, clip)?;
        self.integrate(region, count, limits).map(Some)
    }

    // Collects the fill and the clip paths that do not contain its region,
    // and returns the region's sample count after the source and pixel budgets.
    fn sources(
        &mut self,
        shape: Shape,
        (x, y, width, height): Region,
        limits: Limits,
        clip: Option<&ClipRegion>,
    ) -> Result<usize> {
        self.shapes.clear();
        if limits.mask_sources == 0 {
            return Err(Error::limit("mask source budget exceeded"));
        }
        self.shapes.push(shape);
        for shape in clip.into_iter().flat_map(|clip| &clip.shapes) {
            if !shape.covers((x, y), (width, height)) {
                if self.shapes.len() >= limits.mask_sources {
                    return Err(Error::limit("mask source budget exceeded"));
                }
                self.shapes.push(shape.clone());
            }
        }
        usize::try_from(u64::from(width) * u64::from(height))
            .ok()
            .filter(|&n| n <= limits.pixels)
            .ok_or(Error::limit("mask pixel budget exceeded"))
    }

    fn integrate(
        &mut self,
        (x, y, width, height): Region,
        count: usize,
        limits: Limits,
    ) -> Result<Mask> {
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
        Ok(Mask {
            region: ClipRegion {
                width,
                height,
                origin: (x, y),
                shapes: std::mem::take(&mut self.shapes),
                limits,
            },
            coverage: std::mem::take(&mut self.coverage),
        })
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
        clip: &ClipRegion,
    ) -> Result<()> {
        super::check_clip(target, Some(clip))?;
        let origin = clip.origin;
        if let Some(mask) = self.clipped_path(clip, path, transform, rule, *target.limits())? {
            let offset = (
                mask.origin().0.checked_sub(origin.0),
                mask.origin().1.checked_sub(origin.1),
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

// A device region as `(x, y, width, height)`.
type Region = (u32, u32, u32, u32);

// The fill's shape and its device region within the viewport, if any.
// Only retained fill/clip certificates opt into boundary scan conversion.
// Ordinary paths and stroke outlines take the original raster path.
fn shape(
    path: PathSource<'_>,
    transform: Transform,
    rule: FillRule,
    limits: &Limits,
    (size, origin): ((u32, u32), (u32, u32)),
) -> Result<Option<(Shape, Region)>> {
    let (path, transform, rule) = if path.path().fill_region(rule)
        == Some(crate::FillRegion::EmptyBoundary)
    {
        let outline = crate::empty_fill::outline_boundary(path.path(), transform, rule, limits)?;
        (
            PathSource::Shared(Arc::new(outline)),
            Transform::IDENTITY,
            FillRule::NonZero,
        )
    } else {
        (path, transform, rule)
    };
    let Some(region) = crate::bounds::region(path.path(), transform, size, origin, limits)? else {
        return Ok(None);
    };
    let shape = Shape {
        geometry: Geometry::Path(path.into_shared()),
        transform,
        rule,
    };
    Ok(Some((shape, region)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PathBuilder, Point};

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
                    clip.region(),
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

    type Runs = Vec<(usize, usize, Vec<u8>)>;
    type Placed = Result<Option<((u32, u32), Runs)>>;

    // The closed-form runs and origin, and the integrated mask's.
    fn both(
        clip: &Mask,
        path: &Arc<Path>,
        transform: Transform,
        limits: Limits,
    ) -> (Placed, Placed) {
        let runs = |spans: &mut dyn Iterator<Item = (usize, usize, &[u8])>| -> Runs {
            spans.map(|(y, x, run)| (y, x, run.to_vec())).collect()
        };
        let mut raster = Rasterizer::default();
        let direct = raster
            .clipped_coverage(
                clip.region(),
                Arc::clone(path),
                transform,
                FillRule::NonZero,
                limits,
            )
            .map(|coverage| {
                coverage.map(|coverage| match &coverage {
                    Coverage::Rectangle(rectangle) => {
                        (rectangle.origin(), runs(&mut rectangle.spans()))
                    }
                    Coverage::Mask(_) => panic!("rectangle took the mask"),
                })
            });
        let dense = raster
            .clipped_path(
                clip.region(),
                Arc::clone(path),
                transform,
                FillRule::NonZero,
                limits,
            )
            .map(|mask| mask.map(|mask| (mask.origin(), runs(&mut mask.spans()))));
        (direct, dense)
    }

    #[test]
    fn closed_form_rectangle_runs_equal_mask_runs() {
        let origin = (10, 20);
        let clips = [
            rectangle(0.0, 0.0, 100.0, 100.0),
            rectangle(11.375, 20.5, 3.25, 2.875),
            rectangle(12.0, 19.0, 9.0, 3.0),
            rectangle(13.875, 21.125, 0.25, 0.125),
        ];
        let transforms = [
            Transform::IDENTITY,
            Transform::new(0.0, 1.0, -1.0, 0.0, 30.0, 0.0),
            Transform::new(1.5, 0.0, 0.0, -2.0, 0.0, 47.0),
        ];
        let mut rectangles = 0;
        for clip in &clips {
            let clip = Mask::from_path_at(
                7,
                5,
                clip,
                Transform::IDENTITY,
                FillRule::NonZero,
                35,
                origin,
            )
            .unwrap();
            // Eighth-pixel edges: whole, fractional, sub-pixel and beyond the tile.
            for left in (72..=150).step_by(7) {
                for top in (152..=205).step_by(5) {
                    for (width, height) in [(1, 1), (3, 2), (8, 8), (13, 9), (40, 3), (90, 60)] {
                        let path = rectangle(
                            f64::from(left) / 8.0,
                            f64::from(top) / 8.0,
                            f64::from(width) / 8.0,
                            f64::from(height) / 8.0,
                        );
                        for transform in transforms {
                            let path = Arc::new(path.transformed(transform.inverse().unwrap()));
                            let (direct, dense) = both(&clip, &path, transform, Limits::default());
                            let direct = direct.unwrap();
                            assert_eq!(direct, dense.unwrap());
                            rectangles += usize::from(direct.is_some());
                        }
                    }
                }
            }
        }
        assert!(rectangles > 2000, "{rectangles}");
    }

    #[test]
    fn closed_form_rectangles_charge_and_fail_as_masks() {
        let clip = Mask::from_path_at(
            40,
            30,
            &rectangle(0.0, 0.0, 200.0, 200.0),
            Transform::IDENTITY,
            FillRule::NonZero,
            1200,
            (8, 6),
        )
        .unwrap();
        let path = rectangle(8.5, 6.25, 30.25, 20.5);
        // 31 × 21 samples; the whole fill needs 657 work units in either form.
        let mut outcomes = [0; 2];
        for raster_work in [0, 5, 651, 656, 657, 10_000] {
            for pixels in [650, 651, 10_000] {
                let limits = Limits {
                    raster_work,
                    pixels,
                    ..Limits::default()
                };
                let (direct, dense) = both(&clip, &path, Transform::IDENTITY, limits);
                match (direct, dense) {
                    (Ok(direct), Ok(dense)) => {
                        assert_eq!(direct, dense);
                        outcomes[0] += 1;
                    }
                    (Err(direct), Err(dense)) => {
                        assert_eq!(direct.to_string(), dense.to_string());
                        outcomes[1] += 1;
                    }
                    (direct, dense) => panic!("{raster_work} {pixels}: {direct:?} {dense:?}"),
                }
            }
        }
        assert_eq!(outcomes, [4, 14]);
        // Other shapes and nonrectangular clips keep the integrated mask.
        let mut triangle = PathBuilder::new();
        triangle.move_to(Point::new(9.0, 7.0));
        triangle.line_to(Point::new(30.0, 7.0)).unwrap();
        triangle.line_to(Point::new(9.0, 25.0)).unwrap();
        let triangle = Arc::new(triangle.finish());
        let round = Mask::from_path_at(
            40,
            30,
            &triangle,
            Transform::IDENTITY,
            FillRule::NonZero,
            1200,
            (8, 6),
        )
        .unwrap();
        let mut raster = Rasterizer::default();
        for (clip, path) in [(&clip, &triangle), (&round, &path)] {
            let coverage = raster
                .clipped_coverage(
                    clip.region(),
                    Arc::clone(path),
                    Transform::IDENTITY,
                    FillRule::NonZero,
                    Limits::default(),
                )
                .unwrap()
                .unwrap();
            assert!(matches!(coverage, Coverage::Mask(_)));
            raster.recycle(coverage);
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
                    clip.region(),
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
                clip.region(),
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
                clip.region(),
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
