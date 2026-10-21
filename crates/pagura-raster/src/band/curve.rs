//! Analytic quadratic glyph boundaries (8.5.2.2, 8.5.3.3).
//!
//! A TrueType glyph's quadratics arrive as degree-elevated cubics. Each is
//! recovered and split at both coordinate extrema, so a piece crosses every
//! pixel boundary at most once. Integrating x(t)y'(t) between those
//! crossings gives a cell's area under the true curve, without a polygon.

use super::edges::{Bounds, Line};
use crate::flatten::bounded;
use crate::{Path, Point, Segment, Transform};
use pagura_core::{Error, Limits, Result};

// A cubic is a degree-elevated quadratic exactly when its third difference
// D = -P0 + 3P1 - 3P2 + P3 vanishes. Its midpoint quadratic differs from it by
// D t(t - 1/2)(t - 1), at most |D| sqrt(3)/36, and rounding leaves D at a few
// ulps. Bounding each coordinate of D by 2^-24 pixels keeps that displacement
// below 2^-27: a strip of that width along a monotone arc, which is at most
// two pixels long inside any pixel. 32 pieces then change a pixel by under
// 2^-21 of its area, about 1e-4 levels. General cubics have no such bound, so
// their outlines keep polygons.
const REDUCIBLE: f64 = 1.0 / 16_777_216.0;

/// A quadratic Bézier piece.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Quadratic {
    pub(crate) points: [Point; 3],
}

/// One boundary piece of a glyph in contour order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Piece {
    Line(Line),
    /// A piece monotone in both coordinates.
    Curve(Quadratic),
}

impl Piece {
    pub(crate) fn translated(self, offset: Point) -> Self {
        match self {
            Self::Line(Line { a, b }) => Self::Line(Line {
                a: add(a, offset),
                b: add(b, offset),
            }),
            Self::Curve(q) => Self::Curve(Quadratic {
                points: q.points.map(|p| add(p, offset)),
            }),
        }
    }

    pub(crate) fn ends(self) -> [Point; 2] {
        match self {
            Self::Line(Line { a, b }) => [a, b],
            Self::Curve(q) => [q.points[0], q.points[2]],
        }
    }
}

impl Quadratic {
    pub(crate) fn at(self, t: f64) -> Point {
        let [a, b, c] = self.points;
        lerp(lerp(a, b, t), lerp(b, c, t), t)
    }

    fn coefficients(self) -> [Point; 3] {
        let [a, b, c] = self.points;
        [a, scale(sub(b, a), 2.0), sub(sub(c, b), sub(b, a))]
    }

    fn interval(self, lo: f64, hi: f64) -> Self {
        let [a, b, c] = self.points;
        let tangent = lerp(sub(b, a), sub(c, b), lo);
        let start = self.at(lo);
        Self {
            points: [start, add(start, scale(tangent, hi - lo)), self.at(hi)],
        }
    }

    /// The parameter at which a monotone piece reaches `value` on the x or
    /// y axis, clamped to `[0, 1]`.
    pub(crate) fn parameter(self, value: f64, x_axis: bool) -> f64 {
        let axis = |p: Point| if x_axis { p.x } else { p.y };
        let [p, q, r] = self.points.map(axis);
        if p == r {
            return 0.0;
        }
        // Reflect descending coordinates: the initial derivative is then
        // nonnegative, so this root avoids subtracting nearly equal numbers.
        let sign = (r - p).signum();
        let delta = ((value - p) * sign).clamp(0.0, (r - p).abs());
        if delta == 0.0 {
            return 0.0;
        }
        if delta == (r - p).abs() {
            return 1.0;
        }
        let linear = 2.0 * (q - p) * sign;
        let square = ((r - q) - (q - p)) * sign;
        (2.0 * delta / (linear + (linear * linear + 4.0 * square * delta).max(0.0).sqrt()))
            .clamp(0.0, 1.0)
    }

    // A local polynomial avoids subtracting large antiderivatives on a page
    // translated far from the origin. This is integral (x - column) dy.
    fn integral(self, column: f64) -> f64 {
        let [p, v, a] = self.coefficients();
        (p.x - column) * (self.points[2].y - p.y)
            + v.x * (v.y * 0.5 + a.y * (2.0 / 3.0))
            + a.x * (v.y / 3.0 + a.y * 0.5)
    }

    /// The area of column `column` left of a monotone piece between
    /// parameters `lo` and `hi`.
    pub(crate) fn cell(self, lo: f64, hi: f64, column: f64) -> f64 {
        let a = self.at(lo);
        let b = self.at(hi);
        if a.x == b.x {
            return (b.y - a.y) * (a.x - column).clamp(0.0, 1.0);
        }
        let left = self.parameter(column, true).clamp(lo, hi);
        let right = self.parameter(column + 1.0, true).clamp(lo, hi);
        let (start, end, full) = if a.x < b.x {
            (left, right, b.y - self.at(right).y)
        } else {
            (right, left, self.at(right).y - a.y)
        };
        full + self.interval(start, end).integral(column)
    }
}

/// The monotone pieces of `outline` under the translation-free `linear`
/// map, or `None` unless it is made of lines and quadratics, has curved
/// pieces, no geometric points, and at most `cap` pieces.
pub(crate) fn pieces(
    outline: &Path,
    linear: Transform,
    cap: usize,
    limits: &Limits,
) -> Result<Option<Vec<Piece>>> {
    let mut builder = Builder {
        pieces: Vec::new(),
        cap,
        rejected: false,
        limits,
    };
    builder.path(outline, linear)?;
    Ok((!builder.rejected && builder.pieces.iter().any(|p| matches!(p, Piece::Curve(_))))
        .then_some(builder.pieces))
}

struct Builder<'a> {
    pieces: Vec<Piece>,
    cap: usize,
    rejected: bool,
    limits: &'a Limits,
}

impl Builder<'_> {
    fn push(&mut self, piece: Piece) -> Result<()> {
        if self.pieces.len() >= self.cap {
            self.rejected = true;
            return Ok(());
        }
        if self.pieces.len() >= self.limits.path_segments {
            return Err(Error::limit("quadratic edge budget exceeded"));
        }
        self.pieces
            .try_reserve(1)
            .map_err(|_| Error::limit("glyph piece allocation failed"))?;
        self.pieces.push(piece);
        Ok(())
    }

    fn line(&mut self, a: Point, b: Point) -> Result<()> {
        if a != b {
            self.push(Piece::Line(Line { a, b }))?;
        }
        Ok(())
    }

    fn cubic(&mut self, [a, b, c, d]: [Point; 4]) -> Result<()> {
        if !reducible(sub(sub(d, a), scale(sub(c, b), 3.0))) {
            self.rejected = true;
            return Ok(());
        }
        // Average the controls recovered from each degree-elevated cubic
        // control. This also interpolates the cubic at its midpoint.
        let control = add(lerp(a, d, 0.5), scale(add(sub(b, a), sub(c, d)), 0.75));
        self.quadratic(Quadratic {
            points: [a, control, d],
        })
    }

    fn quadratic(&mut self, curve: Quadratic) -> Result<()> {
        let [_, v, a] = curve.coefficients();
        let mut events = [0.0, 1.0, 0.0, 0.0];
        let mut count = 2;
        for (v, a) in [(v.x, a.x), (v.y, a.y)] {
            if a != 0.0 {
                let t = -v / (2.0 * a);
                if t > 0.0 && t < 1.0 {
                    events[count] = t;
                    count += 1;
                }
            }
        }
        let events = &mut events[..count];
        events.sort_unstable_by(f64::total_cmp);
        for pair in events.windows(2) {
            if pair[0] == pair[1] {
                continue;
            }
            let piece = curve.interval(pair[0], pair[1]);
            // A collinear quadratic's parameterization has no area effect.
            if cross(v, a) == 0.0 {
                self.line(piece.points[0], piece.points[2])?;
            } else if piece.points[0].y != piece.points[2].y {
                self.push(Piece::Curve(piece))?;
            }
        }
        Ok(())
    }

    fn path(&mut self, path: &Path, transform: Transform) -> Result<()> {
        if path.segments().len() > self.limits.path_segments {
            return Err(Error::limit("path segment budget exceeded"));
        }
        let mut start = None;
        let mut current = None;
        let mut source_current = None;
        let mut closed = false;
        let mut degenerate = true;
        let mut segments = 0;
        let map = |point: Point| -> Result<Point> {
            bounded(point)?;
            let point = point * transform;
            bounded(point)?;
            Ok(point)
        };
        for segment in path.segments() {
            if self.rejected {
                return Ok(());
            }
            match *segment {
                Segment::MoveTo(p) => {
                    if let (Some(a), Some(b)) = (start, current)
                        && !closed
                        && segments > 0
                    {
                        self.finish(a, b, degenerate)?;
                    }
                    source_current = Some(p);
                    let p = map(p)?;
                    start = Some(p);
                    current = Some(p);
                    degenerate = true;
                    closed = false;
                    segments = 0;
                }
                Segment::LineTo(p) => {
                    let source = p;
                    let p = map(p)?;
                    let a = current.ok_or(Error::invalid_argument("line has no current point"))?;
                    if closed {
                        start = Some(a);
                        degenerate = true;
                        segments = 0;
                        closed = false;
                    }
                    degenerate &= source_current == Some(source);
                    source_current = Some(source);
                    self.line(a, p)?;
                    current = Some(p);
                    segments += 1;
                }
                Segment::CubicTo { c1, c2, end } => {
                    let a = current.ok_or(Error::invalid_argument("curve has no current point"))?;
                    if closed {
                        start = Some(a);
                        degenerate = true;
                        segments = 0;
                        closed = false;
                    }
                    degenerate &= [c1, c2, end].iter().all(|p| Some(*p) == source_current);
                    source_current = Some(end);
                    let (b, c, d) = (map(c1)?, map(c2)?, map(end)?);
                    self.cubic([a, b, c, d])?;
                    current = Some(d);
                    segments += 1;
                }
                Segment::Close => {
                    if let (Some(a), Some(b)) = (start, current)
                        && !closed
                    {
                        self.finish(a, b, degenerate)?;
                        current = Some(a);
                        closed = true;
                    }
                }
            }
        }
        if let (Some(a), Some(b)) = (start, current)
            && !closed
            && segments > 0
        {
            self.finish(a, b, degenerate)?;
        }
        Ok(())
    }

    fn finish(&mut self, start: Point, current: Point, degenerate: bool) -> Result<()> {
        if degenerate {
            // A geometric point's floor cell keeps the polygon path.
            self.rejected = true;
            return Ok(());
        }
        self.line(current, start)
    }
}

/// Clips monotone pieces to `bounds` as straight edges are clipped: parts
/// above or below disappear and parts beside it become vertical edges on
/// its sides.
pub(crate) fn clip(piece: Quadratic, bounds: Bounds, lines: &mut Vec<Line>, out: &mut Vec<Quadratic>) -> Result<()> {
    if bounds.is_empty() {
        return Ok(());
    }
    let reversed = piece.points[0].y > piece.points[2].y;
    let mut curve = piece;
    if reversed {
        curve.points.reverse();
    }
    let [first, _, last] = curve.points;
    if first.y == last.y {
        return Ok(());
    }
    let reserve = |n: usize, lines: &mut Vec<Line>, out: &mut Vec<Quadratic>| {
        lines
            .try_reserve(n)
            .and_then(|()| out.try_reserve(n))
            .map_err(|_| Error::limit("raster geometry allocation failed"))
    };
    if bounds.left <= first.x.min(last.x)
        && first.x.max(last.x) <= bounds.right
        && bounds.top <= first.y
        && last.y <= bounds.bottom
    {
        reserve(1, lines, out)?;
        out.push(piece);
        return Ok(());
    }
    let lo = first.y.max(bounds.top);
    let hi = last.y.min(bounds.bottom);
    if lo >= hi {
        return Ok(());
    }
    let mut events = [
        curve.parameter(lo, false),
        curve.parameter(hi, false),
        0.0,
        0.0,
    ];
    let mut count = 2;
    for x in [bounds.left, bounds.right] {
        let t = curve.parameter(x, true);
        if t > events[0] && t < events[1] {
            events[count] = t;
            count += 1;
        }
    }
    let events = &mut events[..count];
    events.sort_unstable_by(f64::total_cmp);
    reserve(count, lines, out)?;
    for pair in events.windows(2) {
        let mut part = curve.interval(pair[0], pair[1]);
        let x = part.at(0.5).x;
        if x <= bounds.left || x >= bounds.right {
            let x = x.clamp(bounds.left, bounds.right);
            let (a, b) = (
                Point::new(x, part.points[0].y),
                Point::new(x, part.points[2].y),
            );
            lines.push(if reversed { Line { a: b, b: a } } else { Line { a, b } });
        } else {
            if reversed {
                part.points.reverse();
            }
            out.push(part);
        }
    }
    Ok(())
}

fn reducible(d: Point) -> bool {
    d.x.abs() <= REDUCIBLE && d.y.abs() <= REDUCIBLE
}

fn add(a: Point, b: Point) -> Point {
    Point::new(a.x + b.x, a.y + b.y)
}
fn sub(a: Point, b: Point) -> Point {
    Point::new(a.x - b.x, a.y - b.y)
}
fn scale(a: Point, value: f64) -> Point {
    Point::new(a.x * value, a.y * value)
}
fn lerp(a: Point, b: Point, t: f64) -> Point {
    add(a, scale(sub(b, a), t))
}
fn cross(a: Point, b: Point) -> f64 {
    a.x * b.y - a.y * b.x
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PathBuilder;

    #[test]
    fn elevated_quadratics_split_into_monotone_pieces() {
        // The quadratic (0,0) (2,4) (4,0) as a cubic.
        let mut path = PathBuilder::new();
        path.move_to(Point::new(0.0, 0.0));
        let c = |a: f64, b: f64| a + (b - a) * 2.0 / 3.0;
        path.cubic_to(
            Point::new(c(0.0, 2.0), c(0.0, 4.0)),
            Point::new(c(4.0, 2.0), c(0.0, 4.0)),
            Point::new(4.0, 0.0),
        )
        .unwrap();
        path.close();
        let pieces = pieces(&path.finish(), Transform::IDENTITY, 32, &Limits::default())
            .unwrap()
            .unwrap();
        let curves = pieces
            .iter()
            .filter(|p| matches!(p, Piece::Curve(_)))
            .count();
        assert_eq!(curves, 2);
        // The area under the arch: 2/3 × 4 × 2 = 16/3.
        let area: f64 = pieces
            .iter()
            .map(|p| match *p {
                Piece::Line(Line { a, b }) => (b.y - a.y) * (a.x + b.x) * 0.5,
                Piece::Curve(q) => q.integral(0.0),
            })
            .sum();
        assert!((area.abs() - 16.0 / 3.0).abs() < 1e-12, "{area}");
    }

    #[test]
    fn general_cubics_are_rejected() {
        let mut path = PathBuilder::new();
        path.move_to(Point::new(0.0, 0.0));
        path.cubic_to(
            Point::new(1.0, 3.0),
            Point::new(3.0, -1.0),
            Point::new(4.0, 0.0),
        )
        .unwrap();
        path.close();
        assert!(
            pieces(&path.finish(), Transform::IDENTITY, 32, &Limits::default())
                .unwrap()
                .is_none()
        );
    }
}
