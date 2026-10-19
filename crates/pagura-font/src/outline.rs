//! Budgeted glyph-outline construction, shared by every outline format.
//!
//! This is the only module that calls `PathBuilder`. Each segment is charged
//! to the outline's segment budget before it is added, and every point
//! reaching the path is finite.
use crate::budget::Work;
use crate::read::syntax;
use pagura_core::{Position, Result};
use pagura_raster::{Path, PathBuilder, Point, Segment, Transform};

/// A path under construction, tracking whether a contour is open. Lines
/// and curves need only a current point, which a closed contour keeps.
#[derive(Debug, Default)]
pub(crate) struct Pen {
    path: PathBuilder,
    open: bool,
}

impl Pen {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Whether a contour is open, so that `close` adds a segment.
    pub(crate) const fn is_open(&self) -> bool {
        self.open
    }

    /// The end of the last segment, if any segment was added.
    pub(crate) fn current(&self) -> Option<Point> {
        self.path.current_point()
    }

    /// Closes the open contour, if any, and starts one at `to`.
    pub(crate) fn move_to(&mut self, work: &mut Work<'_>, to: Point, at: Position) -> Result<()> {
        self.close(work, at)?;
        finite(to, at)?;
        work.segments(1, at)?;
        self.path.move_to(to);
        self.open = true;
        Ok(())
    }

    pub(crate) fn line_to(&mut self, work: &mut Work<'_>, to: Point, at: Position) -> Result<()> {
        self.drawing(at)?;
        finite(to, at)?;
        work.segments(1, at)?;
        added(self.path.line_to(to).is_ok(), at)
    }

    pub(crate) fn cubic_to(
        &mut self,
        work: &mut Work<'_>,
        [first, second, to]: [Point; 3],
        at: Position,
    ) -> Result<()> {
        self.drawing(at)?;
        for point in [first, second, to] {
            finite(point, at)?;
        }
        work.segments(1, at)?;
        added(self.path.cubic_to(first, second, to).is_ok(), at)
    }

    /// A quadratic Bézier from the current point, elevated exactly to the
    /// cubic with the same curve.
    pub(crate) fn quad_to(
        &mut self,
        work: &mut Work<'_>,
        control: Point,
        to: Point,
        at: Position,
    ) -> Result<()> {
        let from = self
            .current()
            .ok_or_else(|| syntax(at, "quadratic segment without a current point"))?;
        let third = |end: Point| {
            Point::new(
                end.x + (control.x - end.x) * 2.0 / 3.0,
                end.y + (control.y - end.y) * 2.0 / 3.0,
            )
        };
        self.cubic_to(work, [third(from), third(to), to], at)
    }

    /// Closes the open contour, if any.
    pub(crate) fn close(&mut self, work: &mut Work<'_>, at: Position) -> Result<()> {
        if self.open {
            work.segments(1, at)?;
            self.path.close();
            self.open = false;
        }
        Ok(())
    }

    /// Adds a component outline mapped through `transform`, after closing
    /// any open contour. Components are complete outlines, so no contour is
    /// open afterwards.
    pub(crate) fn append(
        &mut self,
        work: &mut Work<'_>,
        component: &Path,
        transform: Transform,
        at: Position,
    ) -> Result<()> {
        self.close(work, at)?;
        for &segment in component.segments() {
            match segment {
                Segment::MoveTo(p) => self.move_to(work, transform.apply(p), at)?,
                Segment::LineTo(p) => self.line_to(work, transform.apply(p), at)?,
                Segment::CubicTo { c1, c2, end } => {
                    self.cubic_to(work, [c1, c2, end].map(|p| transform.apply(p)), at)?;
                }
                Segment::Close => self.close(work, at)?,
            }
        }
        self.close(work, at)
    }

    /// A segment needs a current point; a closed contour keeps its start.
    fn drawing(&self, at: Position) -> Result<()> {
        added(self.path.current_point().is_some(), at)
    }

    /// The finished outline, mapped through `transform` unless it is the
    /// identity. A transform can make finite points nonfinite; that is a
    /// `Syntax` error at `at`.
    pub(crate) fn finish(mut self, transform: Transform, at: Position) -> Result<Path> {
        let path = self.path.finish();
        if transform == Transform::IDENTITY {
            return Ok(path);
        }
        let path = path.transformed(transform);
        for segment in path.segments() {
            match *segment {
                Segment::MoveTo(p) | Segment::LineTo(p) => finite(p, at)?,
                Segment::CubicTo { c1, c2, end } => {
                    for p in [c1, c2, end] {
                        finite(p, at)?;
                    }
                }
                Segment::Close => {}
            }
        }
        Ok(path)
    }
}

/// A `Syntax` error unless `point` is finite: coordinates beyond `f64`
/// range come only from malformed arithmetic or transforms.
pub(crate) fn finite(point: Point, at: Position) -> Result<()> {
    if point.is_finite() {
        Ok(())
    } else {
        Err(syntax(at, "nonfinite glyph coordinate"))
    }
}

// `PathBuilder` refuses a segment without a current point. A closed
// contour keeps one (its start), as PDF and Type 1 closepath require.
fn added(added: bool, at: Position) -> Result<()> {
    if added {
        Ok(())
    } else {
        Err(syntax(at, "outline segment without a current point"))
    }
}

#[cfg(test)]
mod tests {
    use super::Pen;
    use crate::budget::{FontBudget, Work};
    use pagura_core::{Diagnostics, ErrorKind, Limits, Position, Recovery};
    use pagura_raster::{Point, Segment, Transform};

    #[test]
    fn segments_are_charged_and_contours_close_before_moves() {
        let limits = Limits {
            path_segments: 4,
            ..Limits::default()
        };
        let mut diagnostics = Diagnostics::new(Recovery::Strict);
        let mut work = Work::new(FontBudget::new(&limits), &mut diagnostics);
        let at = Position::Buffer(3);
        let mut pen = Pen::new();
        let error = pen
            .line_to(&mut work, Point::new(1.0, 0.0), at)
            .unwrap_err();
        assert_eq!((error.kind, error.position), (ErrorKind::Syntax, at));
        pen.move_to(&mut work, Point::default(), at).unwrap();
        pen.quad_to(&mut work, Point::new(3.0, 3.0), Point::new(6.0, 0.0), at)
            .unwrap();
        // The second move first closes the open contour: four segments.
        pen.move_to(&mut work, Point::new(9.0, 9.0), at).unwrap();
        assert_eq!(pen.close(&mut work, at).unwrap_err().kind, ErrorKind::Limit);
        let path = pen.finish(Transform::IDENTITY, at).unwrap();
        assert_eq!(
            path.segments()[1],
            Segment::CubicTo {
                c1: Point::new(2.0, 2.0),
                c2: Point::new(4.0, 2.0),
                end: Point::new(6.0, 0.0),
            }
        );
    }

    #[test]
    fn nonfinite_points_are_syntax_errors() {
        let mut diagnostics = Diagnostics::new(Recovery::Strict);
        let mut work = Work::new(FontBudget::new(&Limits::default()), &mut diagnostics);
        let mut pen = Pen::new();
        let error = pen
            .move_to(&mut work, Point::new(f64::INFINITY, 0.0), Position::Unknown)
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax);
    }
}
