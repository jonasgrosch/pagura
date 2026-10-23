//! Path geometry: subpaths of lines and cubic Bézier curves.

use crate::{Point, Transform};
use pagura_core::{Limits, Meter, Rect, Result};
use std::fmt;

/// One path construction step (8.5.2). Exhaustive: every PDF path
/// construction operator reduces to these four (`v`, `y` and `re` included).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Segment {
    /// Begins a new subpath.
    MoveTo(Point),
    /// A straight line from the current point.
    LineTo(Point),
    /// A cubic Bézier curve from the current point.
    CubicTo {
        /// The first control point.
        c1: Point,
        /// The second control point.
        c2: Point,
        /// The end point, which becomes the current point.
        end: Point,
    },
    /// Closes the current subpath with a line to its start.
    Close,
}

/// Which points a fill covers (8.5.3.3). Exhaustive: PDF defines exactly
/// these two rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FillRule {
    /// Nonzero winding number rule.
    #[default]
    NonZero,
    /// Even-odd rule.
    EvenOdd,
}

/// Geometric filling-region certificate retained before scan conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
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
}

impl Path {
    /// A closed rectangle subpath through the corners of `rect`, in the
    /// orientation `re` gives a positive width and height (8.5.2.1).
    pub fn rect(rect: Rect) -> Self {
        let mut builder = PathBuilder::new();
        builder.polygon(Self::rect_corners(rect));
        builder.finish()
    }

    /// [`Self::rect`] with segment capacity charged before construction.
    ///
    /// # Errors
    ///
    /// Returns the meter's `Limit` error if the segment allocation exceeds
    /// its allowance or cannot be reserved.
    pub fn rect_with_bytes(rect: Rect, bytes: &mut Meter) -> Result<Self> {
        let corners = Self::rect_corners(rect);
        let mut builder = PathBuilder::new();
        builder.reserve(corners.len() + 1, bytes)?;
        builder.polygon(corners);
        Ok(builder.finish())
    }

    /// The four corners in the construction order of [`Self::rect`],
    /// without allocating or reconstructing an edge from a width or height.
    pub fn rect_corners(rect: Rect) -> [Point; 4] {
        [
            Point::new(rect.x0(), rect.y0()),
            Point::new(rect.x1(), rect.y0()),
            Point::new(rect.x1(), rect.y1()),
            Point::new(rect.x0(), rect.y1()),
        ]
    }

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
        self.classify_fill_with_bytes(
            rule,
            limits,
            work,
            &mut Meter::new(limits.render_bytes, crate::band::RENDER_BYTES),
        )
    }

    /// [`Self::classify_fill`] with temporary classification storage charged
    /// to the caller's byte meter. Cached certificates need no further work
    /// or storage, and classification scratch is released before returning.
    ///
    /// # Errors
    ///
    /// Returns invalid geometry, exhausted page geometry/work budgets or the
    /// byte meter's `Limit` error if temporary storage cannot be reserved.
    pub fn classify_fill_with_bytes(
        &mut self,
        rule: FillRule,
        limits: &Limits,
        work: &mut usize,
        bytes: &mut Meter,
    ) -> Result<()> {
        let index = usize::from(rule == FillRule::EvenOdd);
        if self.fill_certificates[index].is_none() {
            self.fill_certificates[index] = Some(crate::empty_fill::classify(
                self, rule, limits, work, bytes,
            )?);
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
        let segments = self.mapped(transform).collect();
        Self {
            segments,
            fill_certificates: [None; 2],
        }
    }

    /// A copy charged to `bytes` before its storage is allocated.
    ///
    /// # Errors
    /// Returns the meter's `Limit` error if the allocation exceeds its budget
    /// or cannot be reserved.
    pub fn transformed_with_bytes(&self, transform: Transform, bytes: &mut Meter) -> Result<Self> {
        let mut segments = Vec::new();
        bytes.reserve(&mut segments, self.segments.len())?;
        segments.extend(self.mapped(transform));
        Ok(Self {
            segments,
            fill_certificates: [None; 2],
        })
    }

    /// Copies the stored geometry and certificates without mapping points.
    pub(crate) fn clone_with_bytes(&self, bytes: &mut Meter) -> Result<Self> {
        let mut segments = Vec::new();
        bytes.reserve(&mut segments, self.segments.len())?;
        segments.extend_from_slice(&self.segments);
        Ok(Self {
            segments,
            fill_certificates: self.fill_certificates,
        })
    }

    fn mapped(&self, transform: Transform) -> impl ExactSizeIterator<Item = Segment> + '_ {
        self.segments.iter().map(move |segment| match *segment {
            Segment::MoveTo(p) => Segment::MoveTo(p * transform),
            Segment::LineTo(p) => Segment::LineTo(p * transform),
            Segment::CubicTo { c1, c2, end } => Segment::CubicTo {
                c1: c1 * transform,
                c2: c2 * transform,
                end: end * transform,
            },
            Segment::Close => Segment::Close,
        })
    }

    /// Bytes retained by this path's segment storage.
    pub fn retained_bytes(&self) -> usize {
        self.segments
            .capacity()
            .saturating_mul(size_of::<Segment>())
    }
}

/// A line or curve was added without a current point.
///
/// PDF content (8.5.2.1) and font charstrings treat this as malformed input;
/// each caller reports it with its own error kind and position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NoCurrentPoint;

impl fmt::Display for NoCurrentPoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("path segment has no current point")
    }
}

impl std::error::Error for NoCurrentPoint {}

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

    /// Makes room for the next preparation output before its infallible
    /// construction methods append the already budgeted segments.
    pub(crate) fn reserve(&mut self, count: usize, bytes: &mut Meter) -> Result<()> {
        bytes.reserve_capacity(&mut self.segments, count)
    }

    /// Heap bytes of the output built so far.
    pub(crate) fn heap_bytes(&self) -> usize {
        self.segments
            .capacity()
            .saturating_mul(size_of::<Segment>())
    }

    /// Begins a new subpath at `point` (`m`).
    pub fn move_to(&mut self, point: Point) -> &mut Self {
        self.segments.push(Segment::MoveTo(point));
        self.start = Some(point);
        self.current = Some(point);
        self
    }

    /// Appends a line (`l`).
    ///
    /// # Errors
    ///
    /// Returns [`NoCurrentPoint`] and leaves the builder unchanged if no
    /// subpath is open.
    pub fn line_to(&mut self, point: Point) -> Result<&mut Self, NoCurrentPoint> {
        self.current.ok_or(NoCurrentPoint)?;
        self.segments.push(Segment::LineTo(point));
        self.current = Some(point);
        Ok(self)
    }

    /// Appends a cubic curve (`c`).
    ///
    /// # Errors
    ///
    /// As [`Self::line_to`].
    pub fn cubic_to(
        &mut self,
        c1: Point,
        c2: Point,
        end: Point,
    ) -> Result<&mut Self, NoCurrentPoint> {
        self.current.ok_or(NoCurrentPoint)?;
        self.segments.push(Segment::CubicTo { c1, c2, end });
        self.current = Some(end);
        Ok(self)
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

    /// Appends an open subpath of lines through `points` in order. Adds
    /// nothing for no points.
    pub fn polyline(&mut self, points: impl IntoIterator<Item = Point>) -> &mut Self {
        let mut points = points.into_iter();
        if let Some(first) = points.next() {
            self.move_to(first);
            for point in points {
                self.segments.push(Segment::LineTo(point));
                self.current = Some(point);
            }
        }
        self
    }

    /// Appends a closed polygon subpath through `points` in order. Adds
    /// nothing for no points.
    pub fn polygon(&mut self, points: impl IntoIterator<Item = Point>) -> &mut Self {
        let points = points.into_iter();
        // Room for the points and the close at once (a rectangle is five
        // segments, past the first growth step).
        let (count, _) = points.size_hint();
        if count > 0 {
            self.segments.reserve(count.saturating_add(1));
        }
        self.polyline(points).close()
    }

    /// Appends every subpath of `path`, with each point mapped by
    /// `transform`. A path starts each subpath with [`Segment::MoveTo`], so
    /// the result never depends on this builder's open subpath.
    pub fn append(&mut self, path: &Path, transform: Transform) -> &mut Self {
        for segment in path.segments() {
            match *segment {
                Segment::MoveTo(p) => {
                    self.move_to(p * transform);
                }
                Segment::LineTo(p) => {
                    let p = p * transform;
                    self.segments.push(Segment::LineTo(p));
                    self.current = Some(p);
                }
                Segment::CubicTo { c1, c2, end } => {
                    let end = end * transform;
                    self.segments.push(Segment::CubicTo {
                        c1: c1 * transform,
                        c2: c2 * transform,
                        end,
                    });
                    self.current = Some(end);
                }
                Segment::Close => {
                    self.close();
                }
            }
        }
        self
    }

    /// Finishes the path and resets the builder.
    pub fn finish(&mut self) -> Path {
        self.start = None;
        self.current = None;
        Path {
            segments: std::mem::take(&mut self.segments),
            fill_certificates: [None; 2],
        }
    }
}

#[cfg(test)]
mod tests;
