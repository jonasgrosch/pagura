//! Glyph boundaries integrated against their true curve (8.5.2.2, 8.5.3.3).
//!
//! A TrueType quadratic arrives as a degree-elevated cubic. Its midpoint
//! quadratic recovers it up to rounding; general cubic outlines keep their
//! canonical polygon. Quadratics split at coordinate extrema and integrate
//! x(t)y'(t) between pixel crossings in closed form ([`Mono`]).

use super::edges::{Bounds, Line};
use crate::flatten::bounded;
use crate::{Path, Point, Segment, Transform};
use pagura_core::{Error, Limits, Meter, Result};

/// Maximum sum of absolute cubic third differences per coordinate in one
/// quadratic outline. Bounding the sum, rather than each segment, bounds
/// the entire outline's reduction error independently of its complexity.
const REDUCIBLE: f64 = 1.0 / 16_777_216.0;

/// Whether every curve is quadratic up to the outline's rounding allowance.
/// This defines the reference family from mapped source controls. It is
/// deliberately broader than analytic admission: cancellation at large
/// coordinates can classify an outline here, but the separate rounding
/// bound below must also hold before reducing it to quadratics.
pub(crate) fn quadratic_outline(outline: &Path, linear: Transform) -> bool {
    let (mut start, mut current) = (Point::default(), Point::default());
    let mut difference = Point::default();
    for segment in outline.segments() {
        match *segment {
            Segment::MoveTo(p) => {
                current = p * linear;
                start = current;
            }
            Segment::LineTo(p) => current = p * linear,
            Segment::CubicTo { c1, c2, end } => {
                let [b, c, d] = [c1, c2, end].map(|p| p * linear);
                let third = Point::new(
                    -current.x + 3.0 * b.x - 3.0 * c.x + d.x,
                    -current.y + 3.0 * b.y - 3.0 * c.y + d.y,
                );
                difference.x += third.x.abs();
                difference.y += third.y.abs();
                current = d;
            }
            Segment::Close => current = start,
        }
    }
    difference.x <= REDUCIBLE && difference.y <= REDUCIBLE
}

/// Bounds both the third difference's evaluation and the mapping and
/// midpoint-control arithmetic. The absolute products retain their scale
/// even when a large common offset or a cancelling map hides it in D.
/// Sixty-four rounding units cover the two-term maps, the four-term third
/// difference, and the midpoint quadratic's additions and multiplications.
fn reducible_with_rounding(outline: &Path, linear: Transform) -> bool {
    let (mut start, mut current) = (Point::default(), Point::default());
    let mut error = [0.0_f64; 2];
    for segment in outline.segments() {
        match *segment {
            Segment::MoveTo(p) => {
                current = p;
                start = p;
            }
            Segment::LineTo(p) => current = p,
            Segment::CubicTo { c1, c2, end } => {
                let source = [current, c1, c2, end];
                let [a, b, c, d] = source.map(|p| p * linear);
                let third = [
                    -a.x + 3.0 * b.x - 3.0 * c.x + d.x,
                    -a.y + 3.0 * b.y - 3.0 * c.y + d.y,
                ];
                let products = source.map(|p| {
                    [
                        (p.x * linear.a).abs() + (p.y * linear.c).abs(),
                        (p.x * linear.b).abs() + (p.y * linear.d).abs(),
                    ]
                });
                for axis in 0..2 {
                    let magnitude = products.iter().map(|p| p[axis]).sum::<f64>() + 1.0;
                    let allowance = third[axis].abs() + 64.0 * f64::EPSILON * magnitude;
                    error[axis] = (error[axis] + allowance) * (1.0 + 4.0 * f64::EPSILON);
                }
                current = end;
            }
            Segment::Close => current = start,
        }
    }
    error.into_iter().all(|bound| bound <= REDUCIBLE)
}

/// A quadratic Bézier piece.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Quadratic {
    pub(crate) points: [Point; 3],
}

/// One boundary piece of a glyph in contour order: a straight edge
/// (horizontal ones included, which end chains) or a quadratic monotone in
/// both coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Piece {
    Line(Line),
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
}

/// A monotone quadratic prepared for analytic accumulation, oriented so
/// that y grows with t: x(t) = x0 + t (bx + t ax), y(t) = y0 + t (by + t ay),
/// in coordinates a placement's origin is added to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Mono {
    pub(crate) x0: f64,
    pub(crate) y0: f64,
    pub(crate) x1: f64,
    pub(crate) y1: f64,
    bx: f64,
    ax: f64,
    by: f64,
    ay: f64,
    /// H(t), the integral of (x - x0) dy from 0 to t, is
    /// t^2 (h2 + t (h3 + t h4)).
    h2: f64,
    h3: f64,
    h4: f64,
    /// +1 when the piece runs downward in its own direction, else -1.
    pub(crate) direction: f64,
}

impl Mono {
    /// The integration form of monotone `q`.
    pub(crate) fn new(q: Quadratic) -> Self {
        let (points, direction) = if q.points[0].y <= q.points[2].y {
            (q.points, 1.0)
        } else {
            let [a, b, c] = q.points;
            ([c, b, a], -1.0)
        };
        let [p0, p1, p2] = points;
        let bx = 2.0 * (p1.x - p0.x);
        let ax = (p2.x - p1.x) - (p1.x - p0.x);
        let by = 2.0 * (p1.y - p0.y);
        let ay = (p2.y - p1.y) - (p1.y - p0.y);
        Self {
            x0: p0.x,
            y0: p0.y,
            x1: p2.x,
            y1: p2.y,
            bx,
            ax,
            by,
            ay,
            h2: bx * by * 0.5,
            h3: (2.0 * bx * ay + ax * by) / 3.0,
            h4: ax * ay * 0.5,
            direction,
        }
    }

    /// H(t).
    #[inline]
    pub(crate) fn h(&self, t: f64) -> f64 {
        t * t * (self.h2 + t * (self.h3 + t * self.h4))
    }

    /// x(t) - x0.
    #[inline]
    pub(crate) fn dx(&self, t: f64) -> f64 {
        t * (self.bx + t * self.ax)
    }

    /// y(t) - y0.
    #[inline]
    pub(crate) fn dy(&self, t: f64) -> f64 {
        t * (self.by + t * self.ay)
    }

    /// The parameter where y - y0 reaches `delta`, within `0..=y1 - y0`:
    /// the root whose initial derivative is nonnegative, without
    /// cancellation.
    #[inline]
    pub(crate) fn ty(&self, delta: f64) -> f64 {
        let root = (self.by * self.by + 4.0 * self.ay * delta).max(0.0).sqrt();
        let denominator = self.by + root;
        if denominator > 0.0 {
            (2.0 * delta / denominator).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    /// The parameter where x - x0 reaches `delta`, x monotone.
    #[inline]
    pub(crate) fn tx(&self, delta: f64) -> f64 {
        let (b, a, delta) = if self.x1 >= self.x0 {
            (self.bx, self.ax, delta)
        } else {
            (-self.bx, -self.ax, -delta)
        };
        let root = (b * b + 4.0 * a * delta).max(0.0).sqrt();
        let denominator = b + root;
        if denominator > 0.0 {
            (2.0 * delta / denominator).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    /// x at height `y` within the piece.
    pub(crate) fn x_at(&self, y: f64) -> f64 {
        if y <= self.y0 {
            self.x0
        } else if y >= self.y1 {
            self.x1
        } else {
            self.x0 + self.dx(self.ty(y - self.y0))
        }
    }

    /// The control point.
    pub(crate) fn control(&self) -> Point {
        Point::new(self.x0 + self.bx * 0.5, self.y0 + self.by * 0.5)
    }
}

/// A glyph outline as boundary pieces in contour order: `pieces` from
/// `starts[i]` to the next start are contour `i`.
#[derive(Debug, Default)]
pub(crate) struct Spline {
    pub(crate) pieces: Vec<Piece>,
    pub(crate) starts: Vec<usize>,
}

impl Spline {
    /// The contours' pieces.
    pub(crate) fn contours(&self) -> impl Iterator<Item = &[Piece]> {
        self.starts.iter().enumerate().map(|(i, &start)| {
            let end = self.starts.get(i + 1).copied().unwrap_or(self.pieces.len());
            self.pieces.get(start..end).unwrap_or(&[])
        })
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        (self.pieces.capacity() * size_of::<Piece>())
            .saturating_add(self.starts.capacity() * size_of::<usize>())
    }
}

/// The monotone quadratic pieces of `outline` under `linear`; `None` for
/// general cubics or geometric points, which keep the polygon's point rule
/// (8.5.3.3.1).
///
/// # Errors
///
/// Returns a `Limit` error beyond `limits.path_segments` pieces and
/// `InvalidArgument` for coordinates beyond the working range.
pub(crate) fn spline(outline: &Path, linear: Transform, limits: &Limits) -> Result<Option<Spline>> {
    if outline.segments().len() > limits.path_segments {
        return Err(Error::limit("path segment budget exceeded"));
    }
    if !quadratic_outline(outline, linear) || !reducible_with_rounding(outline, linear) {
        return Ok(None);
    }
    let builder = Builder {
        spline: Spline::default(),
        limits,
    };
    builder.path(outline, linear)
}

struct Builder<'a> {
    spline: Spline,
    limits: &'a Limits,
}

impl Builder<'_> {
    fn push(&mut self, piece: Piece) -> Result<()> {
        if self.spline.pieces.len() >= self.limits.path_segments {
            return Err(Error::limit("quadratic edge budget exceeded"));
        }
        self.spline
            .pieces
            .try_reserve(1)
            .map_err(|_| Error::limit("glyph piece allocation failed"))?;
        self.spline.pieces.push(piece);
        Ok(())
    }

    fn begin(&mut self) -> Result<()> {
        self.spline
            .starts
            .try_reserve(1)
            .map_err(|_| Error::limit("glyph piece allocation failed"))?;
        self.spline.starts.push(self.spline.pieces.len());
        Ok(())
    }

    fn line(&mut self, a: Point, b: Point) -> Result<()> {
        if a != b {
            self.push(Piece::Line(Line { a, b }))?;
        }
        Ok(())
    }

    /// The midpoint quadratic of a degree-elevated cubic. The whole
    /// outline's third differences have already been bounded.
    fn cubic(&mut self, [a, b, c, d]: [Point; 4]) -> Result<()> {
        let control = add(lerp(a, d, 0.5), scale(add(sub(b, a), sub(c, d)), 0.75));
        self.quadratic(Quadratic {
            points: [a, control, d],
        })
    }

    /// A quadratic split at its x and y extrema into monotone pieces whose
    /// shared ends are computed once; an extremum within rounding of an end
    /// is that end.
    fn quadratic(&mut self, curve: Quadratic) -> Result<()> {
        let [_, v, a] = curve.coefficients();
        let [p0, _, p2] = curve.points;
        let mut events = [0.0, 1.0, 0.0, 0.0];
        let mut count = 2;
        for (v, a) in [(v.x, a.x), (v.y, a.y)] {
            if a != 0.0 {
                let t = -v / (2.0 * a);
                if t > EXTREMUM && t < 1.0 - EXTREMUM {
                    events[count] = t;
                    count += 1;
                }
            }
        }
        let events = &mut events[..count];
        events.sort_unstable_by(f64::total_cmp);
        let at = |t: f64| {
            if t == 0.0 {
                p0
            } else if t == 1.0 {
                p2
            } else {
                curve.at(t)
            }
        };
        for pair in events.windows(2) {
            if pair[0] == pair[1] {
                continue;
            }
            let mut piece = curve.interval(pair[0], pair[1]);
            piece.points[0] = at(pair[0]);
            piece.points[2] = at(pair[1]);
            // A collinear quadratic's parameterization has no area effect;
            // a piece without height is horizontal.
            if cross(v, a) == 0.0 || piece.points[0].y == piece.points[2].y {
                self.line(piece.points[0], piece.points[2])?;
            } else {
                self.push(Piece::Curve(piece))?;
            }
        }
        Ok(())
    }

    fn path(mut self, path: &Path, transform: Transform) -> Result<Option<Spline>> {
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
            match *segment {
                Segment::MoveTo(p) => {
                    if let (Some(a), Some(b)) = (start, current)
                        && !closed
                        && segments > 0
                        && !self.finish(a, b, degenerate)?
                    {
                        return Ok(None);
                    }
                    source_current = Some(p);
                    let p = map(p)?;
                    start = Some(p);
                    current = Some(p);
                    degenerate = true;
                    closed = false;
                    segments = 0;
                    self.begin()?;
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
                        self.begin()?;
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
                        self.begin()?;
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
                        if !self.finish(a, b, degenerate)? {
                            return Ok(None);
                        }
                        current = Some(a);
                        closed = true;
                    }
                }
            }
        }
        if let (Some(a), Some(b)) = (start, current)
            && !closed
            && segments > 0
            && !self.finish(a, b, degenerate)?
        {
            return Ok(None);
        }
        Ok(Some(self.spline))
    }

    /// Closes a contour; `false` for a geometric point, which keeps the
    /// polygon's point rule.
    fn finish(&mut self, start: Point, current: Point, degenerate: bool) -> Result<bool> {
        if degenerate {
            return Ok(false);
        }
        self.line(current, start)?;
        Ok(true)
    }
}

/// An extremum this close to an end of a quadratic's parameter range is
/// taken as that end: the piece it would cut off has no extent.
const EXTREMUM: f64 = 1e-9;

/// Clips monotone pieces to `bounds` as straight edges are clipped: parts
/// above or below disappear and parts beside it become vertical edges on
/// its sides.
pub(crate) fn clip(
    piece: Quadratic,
    bounds: Bounds,
    lines: &mut Vec<Line>,
    out: &mut Vec<Quadratic>,
    bytes: &mut Meter,
) -> Result<()> {
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
    if bounds.left <= first.x.min(last.x)
        && first.x.max(last.x) <= bounds.right
        && bounds.top <= first.y
        && last.y <= bounds.bottom
    {
        bytes.reserve_capacity(out, 1)?;
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
    for pair in events.windows(2) {
        let mut part = curve.interval(pair[0], pair[1]);
        let x = part.at(0.5).x;
        if x <= bounds.left || x >= bounds.right {
            let x = x.clamp(bounds.left, bounds.right);
            let (a, b) = (
                Point::new(x, part.points[0].y),
                Point::new(x, part.points[2].y),
            );
            bytes.reserve_capacity(lines, 1)?;
            lines.push(if reversed {
                Line { a: b, b: a }
            } else {
                Line { a, b }
            });
        } else {
            if reversed {
                part.points.reverse();
            }
            bytes.reserve_capacity(out, 1)?;
            out.push(part);
        }
    }
    Ok(())
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

    /// The integral of x dy along the pieces: the signed area they bound.
    fn area(spline: &Spline) -> f64 {
        spline
            .pieces
            .iter()
            .map(|p| match *p {
                Piece::Line(Line { a, b }) => (b.y - a.y) * (a.x + b.x) * 0.5,
                Piece::Curve(q) => {
                    let m = Mono::new(q);
                    m.direction * ((m.y1 - m.y0) * m.x0 + m.h(1.0))
                }
            })
            .sum()
    }

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
        let spline = spline(&path.finish(), Transform::IDENTITY, &Limits::default())
            .unwrap()
            .unwrap();
        let curves = spline
            .pieces
            .iter()
            .filter(|p| matches!(p, Piece::Curve(_)))
            .count();
        assert_eq!(curves, 2);
        // The area under the arch, 2/3 × 4 × 2 = 16/3.
        assert!((area(&spline).abs() - 16.0 / 3.0).abs() < 1e-12);
    }

    #[test]
    fn a_cancelled_third_difference_does_not_hide_mapping_roundoff() {
        let mut path = PathBuilder::new();
        path.move_to(Point::new(1e12, 1e12));
        path.cubic_to(
            Point::new(1e12 + 2.0, 1e12 + 4.0),
            Point::new(1e12 + 4.0, 1e12 + 4.0),
            Point::new(1e12 + 6.0, 1e12),
        )
        .unwrap();
        path.close();
        let path = path.finish();
        assert!(!reducible_with_rounding(&path, Transform::IDENTITY));
        assert!(
            spline(&path, Transform::IDENTITY, &Limits::default())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn general_cubics_keep_the_canonical_polygon() {
        let mut path = PathBuilder::new();
        path.move_to(Point::new(100.0, 0.0));
        path.cubic_to(
            Point::new(100.0, 55.0),
            Point::new(55.0, 100.0),
            Point::new(0.0, 100.0),
        )
        .unwrap();
        path.line_to(Point::new(0.0, 0.0)).unwrap();
        path.close();
        assert!(
            spline(&path.finish(), Transform::IDENTITY, &Limits::default())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn points_are_left_to_the_polygon() {
        let mut path = PathBuilder::new();
        path.move_to(Point::new(1.0, 1.0));
        path.close();
        assert!(
            spline(&path.finish(), Transform::IDENTITY, &Limits::default())
                .unwrap()
                .is_none()
        );
    }
}
