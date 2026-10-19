//! Reusable working storage for a sequence of independent path fills.

use super::{FillRule, Mask};
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
    clip_product: bool,
}

impl Rasterizer {
    /// Creates a rasterizer with explicitly approximate signed winding areas.
    ///
    /// Paths flatten at the supplied device tolerance; prebuilt glyph polygons
    /// retain their vertices and exact positions. Overlap cells may differ from
    /// exact filled-set integration. Complex clip intersections remain exact.
    ///
    /// # Errors
    /// Returns `InvalidArgument` for a tolerance outside (0, 0.25].
    pub fn with_approximate_cells(tolerance: f64) -> Result<Self> {
        if !tolerance.is_finite() || tolerance <= 0.0 || tolerance > 0.25 {
            return Err(Error::invalid_argument("cell tolerance outside (0, 0.25]"));
        }
        let mut raster = Self::default();
        raster.scratch.approximate = Some(tolerance);
        Ok(raster)
    }

    /// Creates signed cells with independent byte coverage multiplication at
    /// clip boundaries. This approximates geometric intersection when both
    /// shapes cross the same pixel. All source geometry remains retained.
    ///
    /// # Errors
    /// As [`Self::with_approximate_cells`].
    pub fn with_approximate_clip_product(tolerance: f64) -> Result<Self> {
        let mut raster = Self::with_approximate_cells(tolerance)?;
        raster.clip_product = true;
        Ok(raster)
    }

    /// Keeps unprepared compound paths on geometric integration. Retained
    /// stroke-boundary proofs, classified fills and glyphs keep the cell policy.
    #[must_use]
    pub fn with_geometric_compounds(mut self) -> Self {
        self.scratch.geometric_compounds = true;
        self
    }

    /// Intersects equal-sized clip masks using this rasterizer's policy.
    /// The default is geometric integration. The explicit clip-product policy
    /// multiplies byte coverage and retains all geometry for later consumers.
    ///
    /// # Errors
    /// Returns incompatible mask sizes or exceeded source/work/allocation limits.
    pub fn intersect_clip(&self, mask: &mut Mask, other: &Mask) -> Result<()> {
        if !self.clip_product {
            return mask.intersect(other);
        }
        if (mask.width, mask.height, mask.origin) != (other.width, other.height, other.origin) {
            return Err(Error::invalid_argument("mask sizes differ"));
        }
        let needed = || {
            other
                .shapes
                .iter()
                .filter(|s| !s.covers(mask.origin, (mask.width, mask.height)))
        };
        let additional = needed().count();
        if additional == 0 {
            return Ok(());
        }
        let count = mask
            .shapes
            .len()
            .checked_add(additional)
            .filter(|&n| n <= mask.limits.mask_sources)
            .ok_or(Error::limit("mask intersection count budget exceeded"))?;
        if mask.coverage.len().saturating_add(count) > mask.limits.raster_work {
            return Err(Error::limit("clip product work exceeded"));
        }
        let mut shapes = Vec::new();
        shapes
            .try_reserve_exact(count)
            .map_err(|_| Error::limit("mask geometry allocation failed"))?;
        shapes.extend(mask.shapes.iter().cloned());
        shapes.extend(needed().cloned());
        multiply_mask(&mut mask.coverage, &other.coverage);
        mask.shapes = shapes;
        Ok(())
    }

    /// Estimated bytes retained in working buffers, including vector capacities.
    /// Returned masks and their shared paths belong to the caller and are excluded.
    pub fn retained_bytes(&self) -> usize {
        self.scratch_bytes
            .saturating_add(self.coverage.capacity())
            .saturating_add(self.shapes.capacity().saturating_mul(size_of::<Shape>()))
    }

    /// Releases all working buffers. It does not affect previously returned masks.
    pub fn clear(&mut self) {
        let approximate = self.scratch.approximate;
        let geometric_compounds = self.scratch.geometric_compounds;
        let clip_product = self.clip_product;
        *self = Self::default();
        self.scratch.approximate = approximate;
        self.scratch.geometric_compounds = geometric_compounds;
        self.clip_product = clip_product;
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

    /// Rasterizes canonical glyph contours at their exact device origin.
    /// Point classification survives flattening; no coverage is cached.
    /// Returned masks retain shared geometry for later geometric intersection.
    ///
    /// # Errors
    /// Returns geometry, work, source-count and allocation limit errors.
    pub fn clipped_glyph(
        &mut self,
        clip: &Mask,
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

    fn complete(&mut self, result: Result<Option<Mask>>) -> Result<Option<Mask>> {
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
        clip: &Mask,
    ) -> Result<()> {
        super::check_clip(target, Some(clip))?;
        if let Some(mask) = self.clipped_glyph(clip, glyph, origin, *target.limits())? {
            let offset = (mask.origin.0 - clip.origin.0, mask.origin.1 - clip.origin.1);
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
        viewport: ((u32, u32), (u32, u32), Option<&Mask>),
    ) -> Result<Option<Mask>> {
        self.maximum = limits.decoded_bytes;
        self.trim();
        let result = self.fill(path, transform, rule, limits, viewport);
        self.complete(result)
    }

    fn fill(
        &mut self,
        path: PathSource<'_>,
        transform: Transform,
        rule: FillRule,
        limits: Limits,
        (size, origin, clip): ((u32, u32), (u32, u32), Option<&Mask>),
    ) -> Result<Option<Mask>> {
        // Only retained fill/clip certificates opt into boundary scan conversion.
        // Ordinary paths and stroke outlines take the original raster path.
        let (path, transform, rule) =
            if path.path().fill_region(rule) == Some(crate::FillRegion::EmptyBoundary) {
                let outline =
                    crate::empty_fill::outline_boundary(path.path(), transform, rule, &limits)?;
                (
                    PathSource::Shared(Arc::new(outline)),
                    Transform::IDENTITY,
                    FillRule::NonZero,
                )
            } else {
                (path, transform, rule)
            };
        let Some((x, y, width, height)) =
            crate::bounds::region(path.path(), transform, size, origin, &limits)?
        else {
            return Ok(None);
        };
        self.fill_shape(
            Shape {
                geometry: Geometry::Path(path.into_shared()),
                transform,
                rule,
            },
            (x, y, width, height),
            limits,
            clip,
        )
    }

    fn fill_shape(
        &mut self,
        shape: Shape,
        (x, y, width, height): (u32, u32, u32, u32),
        limits: Limits,
        clip: Option<&Mask>,
    ) -> Result<Option<Mask>> {
        self.shapes.clear();
        if limits.mask_sources == 0 {
            return Err(Error::limit("mask source budget exceeded"));
        }
        let hull = if self.scratch.approximate.is_some() {
            admission_hull(&shape, &limits)?
        } else {
            None
        };
        self.shapes.push(shape);
        for shape in clip.into_iter().flat_map(|clip| &clip.shapes) {
            if !hull.is_some_and(|hull| shape.contains_hull(hull))
                && !shape.covers((x, y), (width, height))
            {
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
        self.clipped_coverage(clip, (x, y), (width, height), limits)?;
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

    fn clipped_coverage(
        &mut self,
        clip: Option<&Mask>,
        (x, y): (u32, u32),
        (width, height): (u32, u32),
        limits: Limits,
    ) -> Result<()> {
        let count = self.coverage.len();
        let scan_charge = count.checked_mul(2).filter(|&n| n <= limits.raster_work);
        let (binary, scan_charge) = if self.scratch.approximate.is_some() && self.shapes.len() > 1 {
            if let (Some(clip), Some(charge)) = (clip, scan_charge) {
                for shape in &self.shapes {
                    if shape.segments() > limits.path_segments {
                        return Err(Error::limit("path segment budget exceeded"));
                    }
                }
                (
                    self.clip_product || binary_clip(clip, (x, y), (width, height))?,
                    charge,
                )
            } else {
                (false, 0)
            }
        } else {
            (false, 0)
        };
        crate::coverage::coverage(
            width,
            height,
            if binary {
                &self.shapes[..1]
            } else {
                &self.shapes
            },
            &mut self.coverage,
            (x, y),
            &Limits {
                raster_work: limits.raster_work - scan_charge,
                ..limits
            },
            &mut self.scratch,
        )?;
        if binary && let Some(clip) = clip {
            for row in 0..height as usize {
                let source = clip_row(clip, (x, y), width as usize, row)?;
                let dest = &mut self.coverage[row * width as usize..(row + 1) * width as usize];
                for (out, &mask) in dest.iter_mut().zip(source) {
                    *out = if self.clip_product {
                        // A convex product of two bytes remains a byte.
                        u8::try_from((u16::from(*out) * u16::from(mask) + 127) / 255)
                            .map_err(|_| Error::invalid_argument("clip product overflow"))?
                    } else {
                        *out & mask
                    };
                }
            }
        }
        Ok(())
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

#[expect(
    clippy::cast_possible_truncation,
    reason = "a rounded byte product is at most 255"
)]
fn multiply_mask(target: &mut [u8], coverage: &[u8]) {
    let (blocks, tail) = target.as_chunks_mut::<16>();
    let (masks, mask_tail) = coverage.as_chunks::<16>();
    for (block, mask) in blocks.iter_mut().zip(masks) {
        if *mask == [255; 16] {
            continue;
        }
        if *mask == [0; 16] {
            block.fill(0);
            continue;
        }
        for (out, &mask) in block.iter_mut().zip(mask) {
            *out = ((u16::from(*out) * u16::from(mask) + 127) / 255) as u8;
        }
    }
    for (out, &mask) in tail.iter_mut().zip(mask_tail) {
        *out = ((u16::from(*out) * u16::from(mask) + 127) / 255) as u8;
    }
}

// Global control bounds precede tile cropping. Floor-cell expansion retains
// the point-subpath policy before proving a convex clip redundant.
fn admission_hull(shape: &Shape, limits: &Limits) -> Result<Option<[crate::Point; 2]>> {
    Ok(match &shape.geometry {
        Geometry::Glyph(glyph) => glyph
            .bounds()
            .filter(|_| {
                !glyph
                    .contours()
                    .iter()
                    .any(crate::GlyphContour::is_degenerate)
            })
            .map(|bounds| bounds.map(|p| p * shape.transform)),
        Geometry::Path(path) => {
            let [min, max] = crate::bounds::control_hull(path, shape.transform, limits)?;
            (min.x <= max.x && min.y <= max.y).then_some([
                crate::Point::new(min.x.floor(), min.y.floor()),
                crate::Point::new(max.x.floor() + 1.0, max.y.floor() + 1.0),
            ])
        }
    })
}

// The approximate policy may resolve a binary byte clip without another
// multi-source sweep. A rounded-full clip can hide less than half a level of
// area; fractional clip cells retain the exact geometric intersection path.
fn binary_clip(clip: &Mask, origin: (u32, u32), size: (u32, u32)) -> Result<bool> {
    for row in 0..size.1 as usize {
        if !clip_row(clip, origin, size.0 as usize, row)?
            .iter()
            .all(|&v| v == 0 || v == 255)
        {
            return Ok(false);
        }
    }
    Ok(true)
}
fn clip_row(clip: &Mask, origin: (u32, u32), width: usize, row: usize) -> Result<&[u8]> {
    let x = origin
        .0
        .checked_sub(clip.origin.0)
        .ok_or(Error::invalid_argument("mask precedes clip"))? as usize;
    let y = origin
        .1
        .checked_sub(clip.origin.1)
        .ok_or(Error::invalid_argument("mask precedes clip"))? as usize;
    let start = y
        .checked_add(row)
        .and_then(|v| v.checked_mul(clip.width as usize))
        .and_then(|n| n.checked_add(x))
        .ok_or(Error::limit("clip row extent overflow"))?;
    if x.checked_add(width)
        .is_none_or(|end| end > clip.width as usize)
    {
        return Err(Error::invalid_argument("mask exceeds clip width"));
    }
    let end = start
        .checked_add(width)
        .ok_or(Error::limit("clip row extent overflow"))?;
    clip.coverage
        .get(start..end)
        .ok_or(Error::invalid_argument("mask exceeds clip height"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn portable_mask_products_keep_every_tail_and_uniform_run() {
        for size in 0..=97 {
            for mask in [0, 1, 127, 128, 254, 255] {
                let mut pixels: Vec<_> = (0..size)
                    .map(|i| u8::try_from((i * 73 + 29) % 256).unwrap())
                    .collect();
                let source: Vec<_> = (0..size)
                    .map(|i| match (i / 16) % 3 {
                        0 => 255,
                        1 => 0,
                        _ => mask,
                    })
                    .collect();
                let expected: Vec<_> = pixels
                    .iter()
                    .zip(&source)
                    .map(|(&a, &b)| {
                        u8::try_from((u32::from(a) * u32::from(b) + 127) / 255).unwrap()
                    })
                    .collect();
                multiply_mask(&mut pixels, &source);
                assert_eq!(pixels, expected);
            }
        }
    }
    use crate::PathBuilder;

    fn rectangle(x: f64, y: f64, width: f64, height: f64) -> Arc<Path> {
        let mut path = PathBuilder::new();
        path.rect(x, y, width, height);
        Arc::new(path.finish())
    }

    #[test]
    fn convex_clip_admission_uses_global_path_cells() {
        let mut diamond = PathBuilder::new();
        diamond.move_to(crate::Point::new(8.0, 0.0));
        diamond.line_to(crate::Point::new(16.0, 8.0));
        diamond.line_to(crate::Point::new(8.0, 16.0));
        diamond.line_to(crate::Point::new(0.0, 8.0));
        diamond.close();
        let clip = Mask::from_path(
            16,
            16,
            &diamond.finish(),
            Transform::IDENTITY,
            FillRule::NonZero,
            256,
        )
        .unwrap();
        let mut raster = Rasterizer::with_approximate_cells(1.0 / 64.0).unwrap();
        let mask = raster
            .clipped_path(
                &clip,
                rectangle(6.25, 6.25, 2.5, 2.5),
                Transform::IDENTITY,
                FillRule::NonZero,
                Limits::default(),
            )
            .unwrap()
            .unwrap();
        assert_eq!(mask.shapes.len(), 1);
        assert_eq!(
            mask.coverage(),
            [143, 191, 143, 191, 255, 191, 143, 191, 143]
        );
        raster.recycle(mask);
        // This global hull reaches a clip edge; the clip must remain even if
        // one tile happens to contain only its interior part.
        let mask = raster
            .clipped_path(
                &clip,
                rectangle(0.0, 0.0, 8.0, 8.0),
                Transform::IDENTITY,
                FillRule::NonZero,
                Limits::default(),
            )
            .unwrap()
            .unwrap();
        assert_eq!(mask.shapes.len(), 2);
        assert_eq!(mask.coverage()[0], 0);
    }

    #[test]
    fn binary_clip_rows_preserve_nonconvex_mask_geometry() {
        let mut path = PathBuilder::new();
        path.rect(0.0, 0.0, 4.0, 2.0);
        path.rect(0.0, 2.0, 2.0, 2.0);
        let clip = Mask::from_path(
            4,
            4,
            &path.finish(),
            Transform::IDENTITY,
            FillRule::NonZero,
            16,
        )
        .unwrap();
        assert!(binary_clip(&clip, (0, 0), (4, 4)).unwrap());
        let mut raster = Rasterizer::with_approximate_cells(1.0 / 64.0).unwrap();
        let mask = raster
            .clipped_path(
                &clip,
                rectangle(0.0, 0.0, 4.0, 4.0),
                Transform::IDENTITY,
                FillRule::NonZero,
                Limits::default(),
            )
            .unwrap()
            .unwrap();
        assert_eq!(mask.shapes.len(), 2);
        assert_eq!(
            mask.coverage(),
            [
                255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 0, 0, 255, 255, 0, 0
            ]
        );
        let fractional = Mask::from_path(
            4,
            4,
            &rectangle(0.25, 0.25, 3.5, 3.5),
            Transform::IDENTITY,
            FillRule::NonZero,
            16,
        )
        .unwrap();
        assert!(!binary_clip(&fractional, (0, 0), (4, 4)).unwrap());
    }

    #[test]
    fn explicit_clip_product_retains_geometry_and_declares_edge_error() {
        let clip = Mask::from_path(
            2,
            2,
            &rectangle(0.5, 0.0, 1.0, 2.0),
            Transform::IDENTITY,
            FillRule::NonZero,
            16,
        )
        .unwrap();
        let source = rectangle(0.25, 0.0, 1.5, 2.0);
        let mut product = Rasterizer::with_approximate_clip_product(1.0 / 64.0).unwrap();
        for _ in 0..2 {
            let mask = product
                .clipped_path(
                    &clip,
                    source.clone(),
                    Transform::IDENTITY,
                    FillRule::NonZero,
                    Limits::default(),
                )
                .unwrap()
                .unwrap();
            // Each independent shape covers 3/4 and 1/2 of this pixel.
            assert_eq!(mask.coverage(), &[96; 4]);
            assert_eq!(mask.shapes.len(), 2);
            product.clear();
        }
        let mut geometric = Rasterizer::with_approximate_cells(1.0 / 64.0).unwrap();
        let mask = geometric
            .clipped_path(
                &clip,
                source,
                Transform::IDENTITY,
                FillRule::NonZero,
                Limits::default(),
            )
            .unwrap()
            .unwrap();
        // Their actual geometric intersection occupies one half, not 3/8.
        assert_eq!(mask.coverage(), &[128; 4]);
        let mut repeated = clip.clone();
        product.intersect_clip(&mut repeated, &clip).unwrap();
        assert_eq!(repeated.coverage(), &[64; 4]);
        assert_eq!(repeated.shapes.len(), 2);
        let full = Mask::from_path(
            2,
            2,
            &rectangle(0.0, 0.0, 2.0, 2.0),
            Transform::IDENTITY,
            FillRule::NonZero,
            16,
        )
        .unwrap();
        let mut unchanged = clip.clone();
        unchanged.limits.raster_work = 0;
        let expected = unchanged.clone();
        product.intersect_clip(&mut unchanged, &full).unwrap();
        assert_eq!(unchanged, expected);
    }

    #[test]
    fn geometric_compounds_integrate_the_union_before_folding() {
        let clip = Mask::from_path(
            2,
            1,
            &rectangle(0.0, 0.0, 2.0, 1.0),
            Transform::IDENTITY,
            FillRule::NonZero,
            16,
        )
        .unwrap();
        let mut path = PathBuilder::new();
        path.rect(0.25, 0.0, 1.0, 1.0);
        path.rect(0.75, 0.0, 1.0, 1.0);
        let path = Arc::new(path.finish());
        let mut cells = Rasterizer::with_approximate_cells(1.0 / 64.0).unwrap();
        let mask = cells
            .clipped_path(
                &clip,
                path.clone(),
                Transform::IDENTITY,
                FillRule::NonZero,
                Limits::default(),
            )
            .unwrap()
            .unwrap();
        assert_eq!(mask.coverage(), [255; 2]);
        let mut hybrid = Rasterizer::with_approximate_cells(1.0 / 64.0)
            .unwrap()
            .with_geometric_compounds();
        for _ in 0..2 {
            let mask = hybrid
                .clipped_path(
                    &clip,
                    path.clone(),
                    Transform::IDENTITY,
                    FillRule::NonZero,
                    Limits::default(),
                )
                .unwrap()
                .unwrap();
            assert_eq!(mask.coverage(), [191; 2]);
            hybrid.clear();
        }
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
