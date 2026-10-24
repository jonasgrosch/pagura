//! Glyph boundaries integrated against their true curve (8.5.2.2, 8.5.3.3).
//!
//! A TrueType quadratic arrives as itself ([`Segment::QuadTo`]). A cubic
//! that is a degree-elevated quadratic is recovered by its midpoint
//! quadratic up to rounding; general cubic outlines keep their canonical
//! polygon. Quadratics split at coordinate extrema and integrate x(t)y'(t)
//! between pixel crossings in closed form ([`Mono`]).

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
            Segment::QuadTo { end, .. } => current = end * linear,
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
    reduction_errors(outline, linear)
        .into_iter()
        .all(|bound| bound <= REDUCIBLE)
}

/// Local area swept by the midpoint reduction, including mapping and
/// control construction rounding. Each cubic coordinate has at most three
/// monotone intervals in a unit pixel (ADR 0004 §4).
pub(crate) fn reduction_area(outline: &Path, linear: Transform) -> f64 {
    let [x, y] = reduction_errors(outline, linear);
    3.0 * (x + y) * (1.0 + 8.0 * f64::EPSILON)
}

fn reduction_errors(outline: &Path, linear: Transform) -> [f64; 2] {
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
            // Kept exact: nothing to reduce.
            Segment::QuadTo { end, .. } => current = end,
            Segment::Close => current = start,
        }
    }
    error
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

    /// Error against the exact integral of this piece's stored polynomial
    /// coefficients. Expanded absolute products include constructing H's
    /// coefficients, notably cancellation inside h3, as well as evaluation.
    pub(crate) fn h_error(&self, t: f64) -> f64 {
        let t = t.abs();
        64.0 * f64::EPSILON
            * t
            * t
            * (0.5 * (self.bx * self.by).abs()
                + t * ((2.0 * self.bx * self.ay).abs() + (self.ax * self.by).abs()) / 3.0
                + t * t * 0.5 * (self.ax * self.ay).abs())
            + f64::MIN_POSITIVE
    }

    /// A placement-independent admission allowance for direct accumulation.
    /// On a monotone piece, roots are bounded through their coordinate
    /// residuals, rather than an inverse derivative. Coordinate operations
    /// scale with the coefficients; H, offset-height products and the sum
    /// of column heights scale at most quadratically. The broad allowance
    /// includes both ends, all columns in one row, and coefficient setup.
    /// Large pieces still use the measured per-pixel certificate.
    pub(crate) fn arithmetic_allowance(&self) -> f64 {
        let scale = 1.0 + self.bx.abs() + self.ax.abs() + self.by.abs() + self.ay.abs();
        4096.0 * f64::EPSILON * scale * scale
    }

    /// Error of the cell's antiderivative difference and offset-height
    /// product, before their potentially large cancellation.
    pub(crate) fn area_error(&self, times: [f64; 2], offset: f64, height: f64) -> f64 {
        let [a, b] = times;
        self.h_error(a)
            + self.h_error(b)
            + 32.0 * f64::EPSILON * ((offset * height).abs() + self.h(a).abs() + self.h(b).abs())
    }

    /// Error of one coordinate's Horner evaluation against the exact
    /// polynomial with stored coefficients. Source/control construction is
    /// bounded separately by the placement certificate.
    pub(crate) fn coordinate_error(&self, t: f64, x: bool) -> f64 {
        let (b, a) = if x {
            (self.bx, self.ax)
        } else {
            (self.by, self.ay)
        };
        8.0 * f64::EPSILON * ((t * b).abs() + (t * t * a).abs()) + f64::MIN_POSITIVE
    }

    pub(crate) fn height_error(&self, times: [f64; 2]) -> f64 {
        let [a, b] = times;
        self.coordinate_error(a, false)
            + self.coordinate_error(b, false)
            + 8.0 * f64::EPSILON * (self.dy(a).abs() + self.dy(b).abs())
    }

    pub(crate) fn endpoint_error(&self, times: [f64; 2], ends: [f64; 2]) -> f64 {
        let [a, b] = times;
        self.coordinate_error(a, true)
            + self.coordinate_error(b, true)
            + 8.0
                * f64::EPSILON
                * (ends[0].abs() + self.dx(a).abs() + self.dx(b).abs() + ends[1].abs())
    }

    /// A coordinate residual bounds a horizontal/vertical strip directly;
    /// no inverse derivative or well-conditioned root is required.
    pub(crate) fn residual(&self, t: f64, delta: f64, x: bool) -> f64 {
        let value = if x { self.dx(t) } else { self.dy(t) };
        ((value - delta).abs()
            + self.coordinate_error(t, x)
            + 8.0 * f64::EPSILON * (value.abs() + delta.abs()))
            * (1.0 + 16.0 * f64::EPSILON)
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
pub(crate) fn spline(
    outline: &Path,
    linear: Transform,
    limits: &Limits,
    bytes: &mut Meter,
) -> Result<Option<Spline>> {
    spline_using(outline, linear, limits, bytes, PointContours::Keep)
        .map_err(SplineError::into_error)
}

/// A nonpoint projection for a stored polygon with one already classified
/// MoveTo/Close point. Other geometric-point contours still decline.
pub(crate) fn nonpoint_spline(
    outline: &Path,
    linear: Transform,
    limits: &Limits,
    bytes: &mut Meter,
) -> std::result::Result<Option<Spline>, SplineError> {
    spline_using(outline, linear, limits, bytes, PointContours::OmitExplicit)
}

/// Keeps optional byte headroom distinct from geometry/segment failures.
#[derive(Debug)]
pub(crate) enum SplineError {
    Bytes(Error),
    Segments(Error),
    Geometry(Error),
}

impl SplineError {
    pub(crate) fn into_error(self) -> Error {
        match self {
            Self::Bytes(error) | Self::Segments(error) | Self::Geometry(error) => error,
        }
    }
}

impl From<Error> for SplineError {
    fn from(error: Error) -> Self {
        Self::Geometry(error)
    }
}

type BuildResult<T> = std::result::Result<T, SplineError>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum PointContours {
    Keep,
    OmitExplicit,
}

fn spline_using(
    outline: &Path,
    linear: Transform,
    limits: &Limits,
    bytes: &mut Meter,
    point_contours: PointContours,
) -> BuildResult<Option<Spline>> {
    if outline.segments().len() > limits.path_segments {
        return Err(SplineError::Segments(Error::limit(
            "path segment budget exceeded",
        )));
    }
    if !quadratic_outline(outline, linear) || !reducible_with_rounding(outline, linear) {
        return Ok(None);
    }
    let before = bytes.used();
    let builder = Builder {
        spline: Spline::default(),
        limits,
        bytes,
        point_contours,
    };
    let result = builder.path(outline, linear);
    let retained = result
        .as_ref()
        .ok()
        .and_then(Option::as_ref)
        .map_or(0, Spline::retained_bytes);
    bytes.refund(bytes.used().saturating_sub(before.saturating_add(retained)));
    result
}

struct Builder<'a> {
    spline: Spline,
    limits: &'a Limits,
    bytes: &'a mut Meter,
    point_contours: PointContours,
}

impl Builder<'_> {
    fn push(&mut self, piece: Piece) -> BuildResult<()> {
        if self.spline.pieces.len() >= self.limits.path_segments {
            return Err(SplineError::Segments(Error::limit(
                "quadratic edge budget exceeded",
            )));
        }
        self.bytes
            .reserve_capacity(&mut self.spline.pieces, 1)
            .map_err(SplineError::Bytes)?;
        self.spline.pieces.push(piece);
        Ok(())
    }

    fn begin(&mut self) -> BuildResult<()> {
        self.bytes
            .reserve_capacity(&mut self.spline.starts, 1)
            .map_err(SplineError::Bytes)?;
        self.spline.starts.push(self.spline.pieces.len());
        Ok(())
    }

    fn line(&mut self, a: Point, b: Point) -> BuildResult<()> {
        if a != b {
            self.push(Piece::Line(Line { a, b }))?;
        }
        Ok(())
    }

    /// The midpoint quadratic of a degree-elevated cubic. The whole
    /// outline's third differences have already been bounded.
    fn cubic(&mut self, [a, b, c, d]: [Point; 4]) -> BuildResult<()> {
        let control = add(lerp(a, d, 0.5), scale(add(sub(b, a), sub(c, d)), 0.75));
        self.quadratic(Quadratic {
            points: [a, control, d],
        })
    }

    /// A quadratic split at its x and y extrema into monotone pieces whose
    /// shared ends are computed once; an extremum within rounding of an end
    /// is that end.
    fn quadratic(&mut self, curve: Quadratic) -> BuildResult<()> {
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

    fn path(mut self, path: &Path, transform: Transform) -> BuildResult<Option<Spline>> {
        if path.segments().len() > self.limits.path_segments {
            return Err(SplineError::Segments(Error::limit(
                "path segment budget exceeded",
            )));
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
                Segment::LineTo(_) | Segment::CubicTo { .. } | Segment::QuadTo { .. } => {
                    let a =
                        current.ok_or(Error::invalid_argument("segment has no current point"))?;
                    if closed {
                        start = Some(a);
                        degenerate = true;
                        segments = 0;
                        closed = false;
                        self.begin()?;
                    }
                    let (source, end) =
                        self.draw(*segment, (a, source_current), &mut degenerate, map)?;
                    (source_current, current) = (source, Some(end));
                    segments += 1;
                }
                Segment::Close => {
                    if let (Some(a), Some(b)) = (start, current)
                        && !closed
                    {
                        if self.point_contours == PointContours::OmitExplicit
                            && degenerate
                            && segments == 0
                        {
                            // This contour has emitted no piece. Keep no
                            // empty start in the nonpoint projection.
                            self.spline.starts.pop();
                        } else if !self.finish(a, b, degenerate)? {
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
        Ok(
            (self.point_contours == PointContours::Keep || !self.spline.pieces.is_empty())
                .then_some(self.spline),
        )
    }

    /// Adds the pieces of a line or curve from the mapped point `a`, the
    /// source point `from`, and returns its source and mapped ends; a move
    /// or close adds nothing. Clears `degenerate` unless every source
    /// control repeats `from`.
    fn draw(
        &mut self,
        segment: Segment,
        (a, from): (Point, Option<Point>),
        degenerate: &mut bool,
        map: impl Fn(Point) -> Result<Point>,
    ) -> BuildResult<(Option<Point>, Point)> {
        match segment {
            Segment::LineTo(p) => {
                *degenerate &= from == Some(p);
                let d = map(p)?;
                self.line(a, d)?;
                Ok((Some(p), d))
            }
            Segment::CubicTo { c1, c2, end } => {
                *degenerate &= [c1, c2, end].iter().all(|p| Some(*p) == from);
                let (b, c, d) = (map(c1)?, map(c2)?, map(end)?);
                self.cubic([a, b, c, d])?;
                Ok((Some(end), d))
            }
            Segment::QuadTo { control, end } => {
                // A point as the elevated cubic's controls classify it.
                *degenerate &= from.is_some_and(|from| {
                    let [c1, c2] = Segment::elevated(from, control, end);
                    [c1, c2, end].iter().all(|p| *p == from)
                });
                let (b, d) = (map(control)?, map(end)?);
                self.quadratic(Quadratic { points: [a, b, d] })?;
                Ok((Some(end), d))
            }
            Segment::MoveTo(_) | Segment::Close => Ok((from, a)),
        }
    }

    /// Closes a contour; `false` for a geometric point, which keeps the
    /// polygon's point rule.
    fn finish(&mut self, start: Point, current: Point, degenerate: bool) -> BuildResult<bool> {
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
mod tests;
