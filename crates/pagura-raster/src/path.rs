//! Path geometry: subpaths of lines and cubic Bézier curves.

use crate::{FillRule, Point, Transform};
use pagura_core::{Limits, Result};
use std::sync::Arc;

/// One path construction step (ISO 32000-2 clause 8.5.2 semantics).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Segment {
    /// Begins a new subpath.
    MoveTo(Point),
    /// A straight line from the current point.
    LineTo(Point),
    /// A cubic Bézier curve: two control points, then the end point.
    CubicTo(Point, Point, Point),
    /// Closes the current subpath with a line to its start.
    Close,
}

/// Geometric filling-region certificate retained before scan conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillRegion {
    /// Ordinary filling, including geometry whose emptiness cannot be proved.
    Ordinary,
    /// Geometric points retain the existing floor-cell scan conversion.
    Points,
    /// Empty interior with a nonpoint boundary, painted as device hairlines.
    EmptyBoundary,
}

/// An immutable path. Build one with [`PathBuilder`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Path {
    segments: Vec<Segment>,
    fill_certificates: [Option<FillRegion>; 2],
    // A stroke outline's proven equivalent without overlapping parts: the two
    // boundaries of a convex closed run, or pieces that meet without
    // overlapping. The strip sweep uses it only where it admits it in these
    // components' frame; signed cells integrate it in place of the outline.
    pub(crate) boundary: Option<Arc<Path>>,
}

impl Path {
    /// The segments in construction order. Every subpath starts with
    /// [`Segment::MoveTo`].
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// Whether the path has no segments.
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// Certifies an empty filling region once, before retaining a fill or clip.
    /// `work` is the caller's cumulative page work counter. Uncertain geometry
    /// keeps ordinary filling. Transforming geometry clears this certificate.
    ///
    /// # Errors
    /// Returns invalid geometry or exhausted page geometry/work budgets.
    pub fn classify_fill(
        &mut self,
        rule: FillRule,
        limits: &Limits,
        work: &mut usize,
    ) -> Result<()> {
        let index = usize::from(rule == FillRule::EvenOdd);
        if self.fill_certificates[index].is_none() {
            self.fill_certificates[index] =
                Some(crate::empty_fill::classify(self, rule, limits, work)?);
        }
        Ok(())
    }

    /// The retained certificate, or `None` for unprepared geometry. Ordinary
    /// raster paths and all stroke outlines remain unprepared.
    pub fn fill_region(&self, rule: FillRule) -> Option<FillRegion> {
        self.fill_certificates[usize::from(rule == FillRule::EvenOdd)]
    }

    /// A copy with every point mapped by `transform`.
    #[must_use]
    pub fn transformed(&self, transform: Transform) -> Self {
        let map = |point: Point| transform.apply(point);
        let segments = self
            .segments
            .iter()
            .map(|segment| match *segment {
                Segment::MoveTo(p) => Segment::MoveTo(map(p)),
                Segment::LineTo(p) => Segment::LineTo(map(p)),
                Segment::CubicTo(a, b, p) => Segment::CubicTo(map(a), map(b), map(p)),
                Segment::Close => Segment::Close,
            })
            .collect();
        Self {
            segments,
            fill_certificates: [None; 2],
            boundary: None,
        }
    }
}

/// Builds a [`Path`] with PDF's implicit-subpath rules.
#[derive(Debug, Clone, Default)]
pub struct PathBuilder {
    segments: Vec<Segment>,
    start: Option<Point>,
    current: Option<Point>,
}

impl PathBuilder {
    /// Creates an empty builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// The current point, if a subpath is open.
    pub fn current_point(&self) -> Option<Point> {
        self.current
    }

    /// Number of segments so far, for budget checks.
    pub fn len(&self) -> usize {
        self.segments.len()
    }

    /// Whether no segment was added.
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// Begins a new subpath at `point` (`m`).
    pub fn move_to(&mut self, point: Point) -> &mut Self {
        self.segments.push(Segment::MoveTo(point));
        self.start = Some(point);
        self.current = Some(point);
        self
    }

    /// Appends a line (`l`). Returns `None` without a current point, which
    /// PDF content treats as an error.
    pub fn line_to(&mut self, point: Point) -> Option<&mut Self> {
        self.current?;
        self.segments.push(Segment::LineTo(point));
        self.current = Some(point);
        Some(self)
    }

    /// Appends a cubic curve (`c`). Returns `None` without a current point.
    pub fn cubic_to(&mut self, first: Point, second: Point, end: Point) -> Option<&mut Self> {
        self.current?;
        self.segments.push(Segment::CubicTo(first, second, end));
        self.current = Some(end);
        Some(self)
    }

    /// Closes the current subpath (`h`). The current point returns to the
    /// subpath start. Has no effect without an open subpath.
    pub fn close(&mut self) -> &mut Self {
        if self.current.is_some() {
            self.segments.push(Segment::Close);
            self.current = self.start;
        }
        self
    }

    /// Appends a closed rectangle subpath (`re`, clause 8.5.2.1).
    pub fn rect(&mut self, x: f64, y: f64, width: f64, height: f64) -> &mut Self {
        self.move_to(Point::new(x, y));
        self.segments.extend([
            Segment::LineTo(Point::new(x + width, y)),
            Segment::LineTo(Point::new(x + width, y + height)),
            Segment::LineTo(Point::new(x, y + height)),
            Segment::Close,
        ]);
        self
    }

    /// Finishes the path and resets the builder.
    pub fn finish(&mut self) -> Path {
        self.start = None;
        self.current = None;
        Path {
            segments: std::mem::take(&mut self.segments),
            fill_certificates: [None; 2],
            boundary: None,
        }
    }
}
