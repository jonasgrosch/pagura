//! Path geometry: subpaths of lines and cubic Bézier curves.

use crate::{FillRule, Point, Transform};
use pagura_core::{Limits, Result};
use std::{fmt, sync::Arc};

/// One path construction step (ISO 32000-2 clause 8.5.2 semantics).
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
                Segment::CubicTo { c1, c2, end } => Segment::CubicTo {
                    c1: map(c1),
                    c2: map(c2),
                    end: map(end),
                },
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
            boundary: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_and_curves_need_a_current_point() {
        let mut path = PathBuilder::new();
        assert_eq!(
            path.line_to(Point::new(1.0, 0.0)).err(),
            Some(NoCurrentPoint)
        );
        assert_eq!(
            path.cubic_to(Point::default(), Point::default(), Point::default())
                .err(),
            Some(NoCurrentPoint)
        );
        assert!(path.close().is_empty());
        path.move_to(Point::new(1.0, 1.0)).close();
        // Close returns the current point to the subpath start (8.5.2.1).
        path.line_to(Point::new(2.0, 1.0)).unwrap();
        assert_eq!(path.current_point(), Some(Point::new(2.0, 1.0)));
    }

    #[test]
    fn polylines_polygons_and_appended_paths_keep_their_order() {
        let mut path = PathBuilder::new();
        path.polygon([]);
        assert!(path.is_empty());
        let [a, b, c] = [
            Point::new(0.0, 0.0),
            Point::new(2.0, 0.0),
            Point::new(0.0, 2.0),
        ];
        path.polyline([a, b, c]);
        assert_eq!(path.current_point(), Some(c));
        let open = path.finish();
        path.polygon([a, b, c]);
        assert_eq!(path.current_point(), Some(a));
        let triangle = path.finish();
        assert_eq!(
            triangle.segments(),
            [
                Segment::MoveTo(a),
                Segment::LineTo(b),
                Segment::LineTo(c),
                Segment::Close
            ]
        );
        assert_eq!(open.segments(), &triangle.segments()[..3]);
        let mut curve = PathBuilder::new();
        curve.move_to(a).cubic_to(b, c, b).unwrap();
        let curve = curve.finish();
        let shift = Transform::translate(1.0, -1.0);
        let mut combined = PathBuilder::new();
        combined
            .move_to(c)
            .append(&triangle, shift)
            .append(&curve, shift);
        assert_eq!(combined.current_point(), Some(b * shift));
        let expected: Vec<_> = [Segment::MoveTo(c)]
            .into_iter()
            .chain(triangle.transformed(shift).segments().iter().copied())
            .chain(curve.transformed(shift).segments().iter().copied())
            .collect();
        assert_eq!(combined.finish().segments(), expected);
    }
}
