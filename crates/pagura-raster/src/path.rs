//! Path geometry: subpaths of lines and cubic Bézier curves.

use crate::{Point, Transform};

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

/// An immutable path. Build one with [`PathBuilder`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Path {
    segments: Vec<Segment>,
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
        Self { segments }
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
        }
    }
}
