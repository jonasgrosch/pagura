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
    /// Clip regions prepared by [`Self::prepare_clip`] multiply as coverage.
    clip_product: bool,
}

impl Rasterizer {
    /// Creates a rasterizer with explicitly approximate signed winding areas.
    ///
    /// Paths flatten at the supplied device tolerance; prebuilt glyph polygons
    /// retain their vertices and exact positions. Overlap cells may differ from
    /// exact filled-set integration.
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

    /// Creates signed cells that multiply a prepared clip region's own
    /// coverage into each fill (see [`Self::prepare_clip`]). This approximates
    /// geometric intersection where a fill and a clip boundary cross one pixel.
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

    /// Integrates the coverage of each of `clip`'s paths that does not contain
    /// its viewport with signed cells, and keeps their product with the region.
    /// Later fills against the region multiply it instead of integrating the
    /// paths with their own outline. Without the clip-product policy, or when
    /// every path contains the viewport, the region is unchanged.
    ///
    /// # Errors
    /// Returns allocation, geometry, source and work-budget errors.
    pub fn prepare_clip(&mut self, clip: &mut ClipRegion) -> Result<()> {
        (clip.product, clip.rectangle) = (None, None);
        let (origin, size) = (clip.origin, (clip.width, clip.height));
        if !self.clip_product || clip.shapes.iter().all(|s| s.covers(origin, size)) {
            return Ok(());
        }
        // Axis-aligned rectangles intersect into one rectangle, whose
        // coverage is exactly a column factor times a row factor.
        let mut others = false;
        let mut rectangle: Option<[f64; 4]> = None;
        for shape in clip.shapes.iter().filter(|s| !s.covers(origin, size)) {
            if let Some(r) = shape.rectangle() {
                rectangle = Some(rectangle.map_or([r.left, r.top, r.right, r.bottom], |b| {
                    [
                        b[0].max(r.left),
                        b[1].max(r.top),
                        b[2].min(r.right),
                        b[3].min(r.bottom),
                    ]
                }));
            } else {
                others = true;
            }
        }
        clip.rectangle = rectangle.map(|r| r.map(f64::to_bits));
        if !others {
            return Ok(());
        }
        let count = clip.pixels()?;
        let limits = clip.limits;
        self.maximum = limits.decoded_bytes;
        let mut product = Vec::new();
        product
            .try_reserve_exact(count)
            .map_err(|_| Error::limit("clip product allocation failed"))?;
        product.resize(count, 255);
        let result = (|| {
            for shape in clip
                .shapes
                .iter()
                .filter(|s| !s.covers(origin, size) && s.rectangle().is_none())
            {
                self.coverage
                    .try_reserve_exact(count.saturating_sub(self.coverage.len()))
                    .map_err(|_| Error::limit("mask allocation failed"))?;
                self.coverage.clear();
                self.coverage.resize(count, 0);
                crate::coverage::coverage(
                    size.0,
                    size.1,
                    std::slice::from_ref(shape),
                    &mut self.coverage,
                    origin,
                    &limits,
                    &mut self.scratch,
                )?;
                multiply_mask(&mut product, &self.coverage);
            }
            Ok(())
        })();
        self.scratch_bytes = self.scratch.retained_bytes();
        self.complete(result)?;
        clip.product = Some(product.into());
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
            let (count, product) = self.sources(shape, region, limits, Some(clip))?;
            if product {
                return self
                    .integrate_product(region, count, limits, clip)
                    .map(|mask| Some(Coverage::Mask(mask)));
            }
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
        let (count, product) = self.sources(shape, region, limits, clip)?;
        match clip {
            Some(clip) if product => self.integrate_product(region, count, limits, clip),
            _ => self.integrate(region, count, limits),
        }
        .map(Some)
    }

    // Collects the fill and the clip paths that do not contain its region,
    // and returns the region's sample count after the source and pixel
    // budgets, and whether the clip's prepared product applies instead.
    fn sources(
        &mut self,
        shape: Shape,
        (x, y, width, height): Region,
        limits: Limits,
        clip: Option<&ClipRegion>,
    ) -> Result<(usize, bool)> {
        self.shapes.clear();
        if limits.mask_sources == 0 {
            return Err(Error::limit("mask source budget exceeded"));
        }
        let mut shapes = clip.map_or(&[][..], |clip| clip.shapes.as_slice());
        // Approximate cells drop a convex clip that contains the fill's
        // global cells, which no tile can see partly.
        let hull = if self.scratch.approximate.is_some() && !shapes.is_empty() {
            admission_hull(&shape, &limits)?
        } else {
            None
        };
        // A prepared region of rectangles alone is their intersection.
        if let (Some(clip), Some([lo, hi])) = (clip, hull)
            && clip.product.is_none()
            && let Some([left, top, right, bottom]) = clip.rectangle()
            && left <= lo.x
            && top <= lo.y
            && hi.x <= right
            && hi.y <= bottom
        {
            shapes = &[];
        }
        let needed = |clip: &&Shape| {
            !(hull.is_some_and(|hull| clip.contains_hull(hull))
                || clip.covers((x, y), (width, height)))
        };
        // A quadrilateral within quadrilateral clips intersects exactly, and
        // cheaply, by geometry, so a fill drawn over its own clip keeps its
        // edges; any other clip multiplies its prepared product.
        let product = clip.is_some_and(|clip| clip.product.is_some() || clip.rectangle.is_some())
            && shapes.iter().any(|clip| needed(&clip))
            && !(shape.quadrilateral().is_some()
                && shapes
                    .iter()
                    .filter(needed)
                    .all(|clip| clip.quadrilateral().is_some()));
        self.shapes.push(shape);
        if !product {
            for shape in shapes.iter().filter(needed) {
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
        Ok((count, product))
    }

    // Integrates the fill alone and multiplies the clip's prepared product
    // into it; the multiplication is charged as one pass over the region.
    fn integrate_product(
        &mut self,
        region: Region,
        count: usize,
        limits: Limits,
        clip: &ClipRegion,
    ) -> Result<Mask> {
        let raster_work = limits
            .raster_work
            .checked_sub(count)
            .ok_or(Error::limit("clip product work exceeded"))?;
        let mut mask = self.integrate(
            region,
            count,
            Limits {
                raster_work,
                ..limits
            },
        )?;
        let (x, y, width, _) = region;
        if let Some(product) = clip.product.as_deref() {
            let stride = clip.width as usize;
            let left =
                x.checked_sub(clip.origin.0)
                    .ok_or(Error::invalid_argument("mask precedes clip"))? as usize;
            let top =
                y.checked_sub(clip.origin.1)
                    .ok_or(Error::invalid_argument("mask precedes clip"))? as usize;
            for (row, coverage) in mask.coverage.chunks_exact_mut(width as usize).enumerate() {
                let start = (top + row) * stride + left;
                let factors = product
                    .get(start..start + coverage.len())
                    .ok_or(Error::invalid_argument("mask exceeds clip"))?;
                multiply_mask(coverage, factors);
            }
        }
        if let Some(rectangle) = clip.rectangle() {
            clip_rectangle(&mut mask.coverage, (x, y, width), rectangle);
        }
        // Later geometric consumers of the mask still see the clip.
        let (origin, size) = (mask.region.origin, (mask.region.width, mask.region.height));
        for shape in clip.shapes.iter().filter(|s| !s.covers(origin, size)) {
            if mask.region.shapes.len() >= limits.mask_sources {
                return Err(Error::limit("mask source budget exceeded"));
            }
            mask.region.shapes.push(shape.clone());
        }
        Ok(mask)
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
                product: None,
                rectangle: None,
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

/// Multiplies byte coverages, rounding each product once. Full and empty
/// 16-byte blocks of `coverage` take no multiplication.
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

/// Multiplies coverage rows of `width` pixels from device `(x, y)` by the
/// coverage of an axis-aligned rectangle: a column factor times a row
/// factor, rounded once to a byte. Only boundary rows and columns change.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "factors in [0, 1] scale to bytes"
)]
fn clip_rectangle(
    coverage: &mut [u8],
    (x, y, width): (u32, u32, u32),
    [left, top, right, bottom]: [f64; 4],
) {
    let factor = |start: u32, low: f64, high: f64| {
        let start = f64::from(start);
        (high.min(start + 1.0) - low.max(start)).clamp(0.0, 1.0)
    };
    let width = width as usize;
    // Columns whose factor is 1 are those wholly inside the rectangle.
    let full = |column: usize| factor(x + column as u32, left, right) == 1.0;
    for (row, line) in coverage.chunks_exact_mut(width).enumerate() {
        let vertical = factor(y + row as u32, top, bottom);
        if vertical == 0.0 {
            line.fill(0);
            continue;
        }
        for (column, value) in line.iter_mut().enumerate() {
            if vertical == 1.0 && full(column) {
                continue;
            }
            let area = factor(x + column as u32, left, right) * vertical;
            let level = (area * 255.0).round() as u16;
            *value = ((u16::from(*value) * level + 127) / 255) as u8;
        }
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

    fn region(path: &Path, size: u32) -> ClipRegion {
        ClipRegion::from_path(
            size,
            size,
            path,
            Transform::IDENTITY,
            FillRule::NonZero,
            Limits::default(),
            (0, 0),
        )
        .unwrap()
    }

    #[test]
    fn clip_products_keep_rectangles_separable_and_quadrilaterals_geometric() {
        let mut raster = Rasterizer::with_approximate_clip_product(1.0 / 64.0).unwrap();
        // Two rectangles intersect into one: [1.5, 0.25] to [3.25, 2.75].
        let mut clip = region(&rectangle(0.5, 0.25, 2.75, 3.0), 4);
        clip.intersect(&region(&rectangle(1.5, 0.0, 2.5, 2.75), 4))
            .unwrap();
        raster.prepare_clip(&mut clip).unwrap();
        assert_eq!(clip.rectangle(), Some([1.5, 0.25, 3.25, 2.75]));
        assert!(clip.product().is_none());
        // A triangle is not a quadrilateral: its coverage is multiplied by
        // the column factors (1/2, 1, 1/4) and row factors (3/4, 1, 3/4).
        let mut triangle = PathBuilder::new();
        triangle.move_to(Point::new(0.0, 0.0));
        triangle.line_to(Point::new(4.0, 0.0)).unwrap();
        triangle.line_to(Point::new(4.0, 4.0)).unwrap();
        triangle.close();
        let mask = raster
            .clipped_path(
                &clip,
                Arc::new(triangle.finish()),
                Transform::IDENTITY,
                FillRule::NonZero,
                Limits::default(),
            )
            .unwrap()
            .unwrap();
        let alone = Mask::from_path(
            4,
            4,
            &triangle_path(),
            Transform::IDENTITY,
            FillRule::NonZero,
            16,
        )
        .unwrap();
        // Rounded products of the column and row factors.
        let levels = [[0, 96, 191, 48], [0, 128, 255, 64], [0, 96, 191, 48]];
        for (y, levels) in levels.iter().enumerate() {
            for (x, &level) in levels.iter().enumerate().skip(1) {
                let value = u32::from(alone.coverage()[y * 4 + x]);
                let (mx, my) = mask.origin();
                let index = (y - my as usize) * mask.width() as usize + x - mx as usize;
                assert_eq!(
                    u32::from(mask.coverage()[index]),
                    (value * level + 127) / 255,
                    "{x} {y}"
                );
            }
        }
        // A rectangle within the rectangles integrates exactly with them.
        let mask = raster
            .clipped_path(
                &clip,
                rectangle(0.0, 0.5, 4.0, 1.0),
                Transform::IDENTITY,
                FillRule::NonZero,
                Limits::default(),
            )
            .unwrap()
            .unwrap();
        let exact = clip
            .clipped_path(
                &rectangle(0.0, 0.5, 4.0, 1.0),
                Transform::IDENTITY,
                FillRule::NonZero,
                Limits::default(),
            )
            .unwrap()
            .unwrap();
        assert_eq!(mask.coverage(), exact.coverage());
    }

    fn triangle_path() -> Path {
        let mut triangle = PathBuilder::new();
        triangle.move_to(Point::new(0.0, 0.0));
        triangle.line_to(Point::new(4.0, 0.0)).unwrap();
        triangle.line_to(Point::new(4.0, 4.0)).unwrap();
        triangle.close();
        triangle.finish()
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
