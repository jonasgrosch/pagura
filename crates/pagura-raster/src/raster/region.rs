//! Clipping paths kept as geometry until a mark integrates them (8.5.4).

use super::{FillRule, Mask, fill_mask};
use crate::coverage::{Geometry, Shape};
use crate::{Path, Transform};
use pagura_core::{Error, Limits, Result};
use std::sync::Arc;

/// A device viewport restricted by clipping paths (ISO 32000-2, 8.5.4).
///
/// The paths stay geometry. A mark integrates them together with its own
/// outline, exactly as with a [`Mask`], so the region itself costs no coverage
/// until [`Self::mask`] requests it.
#[derive(Clone, PartialEq, Eq)]
pub struct ClipRegion {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) origin: (u32, u32),
    pub(super) shapes: Vec<Shape>,
    pub(super) limits: Limits,
}

/// Summarizes instead of dumping geometry.
impl std::fmt::Debug for ClipRegion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClipRegion")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("origin", &self.origin)
            .field("paths", &self.shapes.len())
            .finish_non_exhaustive()
    }
}

impl ClipRegion {
    /// The area `path` covers under `rule`, in a viewport whose integer origin
    /// is in global device coordinates. Geometry stays in global space, so
    /// every viewport of the same path yields the same pixels.
    ///
    /// # Errors
    /// Returns the viewport, geometry and budget errors of
    /// [`Mask::from_path_with_limits`], without integrating any coverage.
    pub fn from_path(
        width: u32,
        height: u32,
        path: &Path,
        transform: Transform,
        rule: FillRule,
        limits: Limits,
        origin: (u32, u32),
    ) -> Result<Self> {
        if path.segments().len() > limits.path_segments {
            return Err(Error::limit("path segment budget exceeded"));
        }
        let outline = crate::empty_fill::outline(path, transform, rule, &limits)?;
        let (path, transform, rule) = if let Some(outline) = outline {
            (outline, Transform::IDENTITY, FillRule::NonZero)
        } else {
            (path.clone(), transform, rule)
        };
        let shapes = vec![Shape {
            geometry: Geometry::Path(Arc::new(path)),
            transform,
            rule,
        }];
        Self::new(width, height, shapes, limits, origin)
    }

    fn new(
        width: u32,
        height: u32,
        shapes: Vec<Shape>,
        limits: Limits,
        origin: (u32, u32),
    ) -> Result<Self> {
        if shapes.len() > limits.mask_sources {
            return Err(Error::limit("mask source budget exceeded"));
        }
        let region = Self {
            width,
            height,
            origin,
            shapes,
            limits,
        };
        region.pixels()?;
        Ok(region)
    }

    /// Validates the viewport and returns its pixel count.
    pub(super) fn pixels(&self) -> Result<usize> {
        let (width, height) = (self.width, self.height);
        if self.origin.0.checked_add(width).is_none() || self.origin.1.checked_add(height).is_none()
        {
            return Err(Error::limit("tile extent overflows u32"));
        }
        if width == 0 || height == 0 {
            return Err(Error::invalid_argument("mask dimensions must be positive"));
        }
        usize::try_from(u64::from(width) * u64::from(height))
            .ok()
            .filter(|&count| count <= self.limits.pixels)
            .ok_or(Error::limit("mask pixel budget exceeded"))
    }

    /// Global device origin of the viewport.
    pub fn origin(&self) -> (u32, u32) {
        self.origin
    }

    /// Viewport width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Viewport height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Restricts this region to the area also inside `other`. Paths that
    /// contain the whole viewport change no pixel and are not retained.
    ///
    /// # Errors
    /// Returns an invalid-argument error if viewports differ, or a limit
    /// error if the retained paths exceed the mask source budget. The region
    /// is unchanged on error.
    pub fn intersect(&mut self, other: &Self) -> Result<()> {
        if (self.width, self.height, self.origin) != (other.width, other.height, other.origin) {
            return Err(Error::invalid_argument("mask sizes differ"));
        }
        let (origin, size) = (self.origin, (self.width, self.height));
        let needed = || {
            other
                .shapes
                .iter()
                .filter(move |shape| !shape.covers(origin, size))
        };
        let count = needed().count();
        if count == 0 {
            return Ok(());
        }
        self.shapes
            .len()
            .checked_add(count)
            .filter(|&n| n <= self.limits.mask_sources)
            .ok_or(Error::limit("mask intersection count budget exceeded"))?;
        self.shapes
            .try_reserve_exact(count)
            .map_err(|_| Error::limit("mask geometry allocation failed"))?;
        self.shapes.extend(needed().cloned());
        Ok(())
    }

    /// Fills only the path bounds within this region. The clipping paths are
    /// integrated together with `path`, never multiplied as coverage, and the
    /// result retains its global device origin.
    ///
    /// # Errors
    /// Returns geometry and caller-budget errors as [`Mask::from_path_with_limits`].
    pub fn clipped_path(
        &self,
        path: &Path,
        transform: Transform,
        rule: FillRule,
        limits: Limits,
    ) -> Result<Option<Mask>> {
        fill_mask(
            path,
            transform,
            rule,
            Some(self),
            (self.width, self.height),
            self.origin,
            limits,
        )
    }

    /// Integrates the region's own coverage over its viewport.
    ///
    /// # Errors
    /// Returns allocation, geometry and work-budget errors.
    pub fn mask(&self) -> Result<Mask> {
        Mask::integrate(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PathBuilder, Point, Rasterizer};

    fn rectangle(x: f64, y: f64, width: f64, height: f64) -> Path {
        let mut path = PathBuilder::new();
        path.rect(x, y, width, height);
        path.finish()
    }

    fn region(path: &Path, limits: Limits) -> ClipRegion {
        ClipRegion::from_path(
            2,
            2,
            path,
            Transform::IDENTITY,
            FillRule::NonZero,
            limits,
            (10, 20),
        )
        .unwrap()
    }

    #[test]
    fn marks_integrate_clip_paths_without_clip_coverage() {
        let limits = Limits::default();
        let mut clip = region(&rectangle(10.25, 20.25, 1.5, 1.5), limits);
        clip.intersect(&region(&rectangle(10.0, 20.5, 2.0, 1.0), limits))
            .unwrap();
        // Rational areas: the clip is [10.25, 11.75] x [20.5, 21.5], so each
        // pixel holds 3/8, and the mark's [10.75, 11.75] slice 1/8 and 3/8.
        assert_eq!(clip.mask().unwrap().coverage(), [96; 4]);
        let mark = rectangle(10.75, 20.0, 1.0, 2.0);
        let clipped = clip
            .clipped_path(&mark, Transform::IDENTITY, FillRule::NonZero, limits)
            .unwrap()
            .unwrap();
        assert_eq!(
            (clipped.origin(), clipped.coverage()),
            ((10, 20), &[32, 96, 32, 96][..])
        );
        let shared = Rasterizer::default()
            .clipped_path(
                &clip,
                Arc::new(mark),
                Transform::IDENTITY,
                FillRule::NonZero,
                limits,
            )
            .unwrap()
            .unwrap();
        assert_eq!(shared, clipped);
    }

    #[test]
    fn regions_match_integrated_mask_intersections() {
        let limits = Limits::default();
        let mut triangle = PathBuilder::new();
        triangle.move_to(Point::new(10.1, 20.0));
        triangle.line_to(Point::new(12.0, 20.7)).unwrap();
        triangle.line_to(Point::new(10.4, 22.0)).unwrap();
        triangle.close();
        let triangle = triangle.finish();
        let band = rectangle(10.0, 20.3, 2.0, 1.2);
        let dense = |path: &Path| {
            Mask::from_path_with_limits(
                2,
                2,
                path,
                Transform::IDENTITY,
                FillRule::EvenOdd,
                limits,
                (10, 20),
            )
            .unwrap()
        };
        let mut expected = dense(&triangle);
        expected.intersect(&dense(&band)).unwrap();
        let mut actual = ClipRegion::from_path(
            2,
            2,
            &triangle,
            Transform::IDENTITY,
            FillRule::EvenOdd,
            limits,
            (10, 20),
        )
        .unwrap();
        actual.intersect(dense(&band).region()).unwrap();
        assert_eq!(actual, *expected.region());
        assert_eq!(actual.mask().unwrap(), expected);
    }

    #[test]
    fn intersections_keep_only_restricting_paths_within_budget() {
        let limits = Limits {
            mask_sources: 2,
            ..Limits::default()
        };
        let mut clip = region(&rectangle(10.5, 20.5, 1.0, 1.0), limits);
        // A path containing the whole viewport restricts nothing.
        clip.intersect(&region(&rectangle(9.0, 19.0, 4.0, 4.0), limits))
            .unwrap();
        assert_eq!(clip.shapes.len(), 1);
        let partial = region(&rectangle(10.0, 20.0, 1.5, 2.0), limits);
        clip.intersect(&partial).unwrap();
        assert_eq!(clip.shapes.len(), 2);
        let before = clip.clone();
        assert!(clip.intersect(&partial).is_err());
        assert_eq!(clip, before);
        let other = ClipRegion::from_path(
            2,
            2,
            &rectangle(0.0, 0.0, 1.0, 1.0),
            Transform::IDENTITY,
            FillRule::NonZero,
            limits,
            (0, 0),
        )
        .unwrap();
        assert!(clip.intersect(&other).is_err());
        assert_eq!(clip, before);
    }

    #[test]
    fn viewports_are_validated_without_integrating_coverage() {
        let path = rectangle(0.0, 0.0, 1.0, 1.0);
        let small = Limits {
            pixels: 3,
            ..Limits::default()
        };
        let build = |width, height, limits, origin| {
            ClipRegion::from_path(
                width,
                height,
                &path,
                Transform::IDENTITY,
                FillRule::NonZero,
                limits,
                origin,
            )
        };
        assert!(build(2, 2, small, (0, 0)).is_err());
        assert!(build(0, 2, Limits::default(), (0, 0)).is_err());
        assert!(build(2, 2, Limits::default(), (u32::MAX, 0)).is_err());
        assert!(build(1, 3, small, (0, 0)).is_ok());
    }
}
