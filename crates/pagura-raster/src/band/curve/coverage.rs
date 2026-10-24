//! Conservative original-quadratic coverage bounds used only in preparation.
//! Root guesses have validated coordinate residuals; unsupported evidence declines.
//!
//! The original closed path is compared with a monotone quadratic shadow.
//! Outward fees cover mapping, midpoint reduction, literal Builder splits,
//! arc-to-chord collapses and endpoint-preserving middle-control clamps.
//! Their closed-chain bounds are invariant under the common exact translation,
//! so setup uses origin zero; queries retain subtraction tails and point floors.
//!
//! Each shared literal Y slab integrates the complete signed winding prefix.
//! Stable crossings change the nonzero indicator; an uncertain transitive
//! X envelope becomes one nominal step plus a one-sided rectangle enclosure.
//! This permits arbitrary winding without folding its signed integral.
//! Bounded child slabs enclose the same indicator and may narrow a parent by
//! intersection. Source, clamp and physical-window fees enter only once after
//! summing the selected slabs. A caller accepts only a unique coverage byte.

use super::*;

/// A closed real enclosure. Nonfinite endpoints mean unknown evidence.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Interval {
    lo: f64,
    hi: f64,
}

impl Interval {
    pub(crate) const fn exact(value: f64) -> Self {
        Self {
            lo: value,
            hi: value,
        }
    }

    pub(crate) const fn lower(self) -> f64 {
        self.lo
    }
    pub(crate) const fn upper(self) -> f64 {
        self.hi
    }
    pub(crate) fn bounds(lo: f64, hi: f64) -> Self {
        if !lo.is_finite() || !hi.is_finite() || lo > hi {
            unknown()
        } else {
            Self { lo, hi }
        }
    }
    pub(crate) fn finite(self) -> bool {
        self.lo.is_finite() && self.hi.is_finite()
    }
    pub(crate) fn radius(self) -> f64 {
        self.lo.abs().max(self.hi.abs())
    }
    pub(crate) fn widen(self, error: f64) -> Self {
        self.add(Self {
            lo: -error,
            hi: error,
        })
    }

    pub(crate) fn add(self, rhs: Self) -> Self {
        if !self.finite() || !rhs.finite() {
            return unknown();
        }
        Self {
            lo: (self.lo + rhs.lo).next_down(),
            hi: (self.hi + rhs.hi).next_up(),
        }
    }

    pub(crate) fn sub(self, rhs: Self) -> Self {
        if !self.finite() || !rhs.finite() {
            return unknown();
        }
        Self {
            lo: (self.lo - rhs.hi).next_down(),
            hi: (self.hi - rhs.lo).next_up(),
        }
    }

    pub(crate) fn mul(self, rhs: Self) -> Self {
        if !self.finite() || !rhs.finite() {
            return unknown();
        }
        let values = [
            self.lo * rhs.lo,
            self.lo * rhs.hi,
            self.hi * rhs.lo,
            self.hi * rhs.hi,
        ];
        Self {
            lo: values.into_iter().fold(f64::INFINITY, f64::min).next_down(),
            hi: values
                .into_iter()
                .fold(f64::NEG_INFINITY, f64::max)
                .next_up(),
        }
    }

    pub(crate) fn div(self, rhs: Self) -> Self {
        if !self.finite() || !rhs.finite() {
            return unknown();
        }
        if rhs.lo <= 0.0 && rhs.hi >= 0.0 {
            return Self {
                lo: f64::NEG_INFINITY,
                hi: f64::INFINITY,
            };
        }
        let reciprocal = Self {
            lo: (1.0 / rhs.hi).next_down(),
            hi: (1.0 / rhs.lo).next_up(),
        };
        self.mul(reciprocal)
    }
}

/// Optional proof work, isolated from the mandatory preparation meter.
#[derive(Debug)]
pub(crate) struct Work<'a> {
    work: &'a mut Meter,
}

impl<'a> Work<'a> {
    pub(crate) fn new(work: &'a mut Meter) -> Self {
        Self { work }
    }
    pub(crate) fn blocks(&mut self, count: usize) -> Result<()> {
        self.work.charge(count)
    }
}

#[derive(Debug, Clone, Copy)]
enum Axis {
    X,
    Y,
}

#[derive(Debug, Clone, Copy)]
enum Roots {
    Line,
    Curve(Mono),
}

/// Exact-control polynomial coefficients with outward construction error.
#[derive(Debug, Clone, Copy)]
struct Polynomial {
    x: [Interval; 3],
    y: [Interval; 3],
    h: [Interval; 3],
    ends: [Point; 2],
    roots: Roots,
    direction: i64,
}

fn coefficients([a, b, c]: [f64; 3]) -> [Interval; 3] {
    let a = Interval::exact(a);
    let u = Interval::exact(b).sub(a);
    [
        a,
        u.mul(Interval::exact(2.0)),
        Interval::exact(c).sub(Interval::exact(b)).sub(u),
    ]
}

fn value(coefficients: [Interval; 3], t: f64) -> Interval {
    let [a, b, c] = coefficients;
    let t = Interval::exact(t);
    a.add(t.mul(b.add(t.mul(c))))
}

impl Polynomial {
    fn new(piece: Piece, math: &mut Work<'_>) -> Result<Option<Self>> {
        math.blocks(3)?;
        if piece
            .ends()
            .into_iter()
            .any(|p| !p.x.is_finite() || !p.y.is_finite())
        {
            return Ok(None);
        }
        let (mut points, roots) = match piece {
            Piece::Line(Line { a, b }) => ([a, a, b], Roots::Line),
            Piece::Curve(q) => (q.points, Roots::Curve(Mono::new(q))),
        };
        let direction = if points[0].y <= points[2].y { 1 } else { -1 };
        if direction < 0 {
            points.swap(0, 2);
        }
        let [a, b, c] = points;
        let (x, y) = if matches!(roots, Roots::Line) {
            (
                [
                    Interval::exact(a.x),
                    Interval::exact(c.x).sub(Interval::exact(a.x)),
                    Interval::exact(0.0),
                ],
                [
                    Interval::exact(a.y),
                    Interval::exact(c.y).sub(Interval::exact(a.y)),
                    Interval::exact(0.0),
                ],
            )
        } else {
            let ordered_x = (a.x <= b.x && b.x <= c.x) || (a.x >= b.x && b.x >= c.x);
            let ordered_y = a.y <= b.y && b.y <= c.y;
            if !ordered_x || !ordered_y {
                return Ok(None);
            }
            (
                coefficients(points.map(|p| p.x)),
                coefficients(points.map(|p| p.y)),
            )
        };
        let [_, bx, ax] = x;
        let [_, by, ay] = y;
        let half = Interval::exact(0.5);
        let h = [
            bx.mul(by).mul(half),
            bx.mul(ay)
                .mul(Interval::exact(2.0))
                .add(ax.mul(by))
                .div(Interval::exact(3.0)),
            ax.mul(ay).mul(half),
        ];
        Ok(Some(Self {
            x,
            y,
            h,
            ends: [a, c],
            roots,
            direction,
        }))
    }

    pub(crate) fn bottom(&self) -> f64 {
        self.ends[0].y
    }
    pub(crate) fn top(&self) -> f64 {
        self.ends[1].y
    }

    fn coordinate(&self, axis: Axis, t: f64, math: &mut Work<'_>) -> Result<Interval> {
        math.blocks(1)?;
        Ok(value(
            match axis {
                Axis::X => self.x,
                Axis::Y => self.y,
            },
            t,
        ))
    }

    fn integral(&self, t: f64, math: &mut Work<'_>) -> Result<Interval> {
        math.blocks(1)?;
        let [a, b, c] = self.h;
        let t = Interval::exact(t);
        Ok(t.mul(t).mul(a.add(t.mul(b.add(t.mul(c))))))
    }

    fn guess(&self, axis: Axis, target: f64, math: &mut Work<'_>) -> Result<f64> {
        math.blocks(1)?;
        let delta = target
            - match axis {
                Axis::X => self.ends[0].x,
                Axis::Y => self.ends[0].y,
            };
        let t = match self.roots {
            Roots::Curve(m) => match axis {
                Axis::X => m.tx(delta),
                Axis::Y => m.ty(delta),
            },
            Roots::Line => {
                let width = match axis {
                    Axis::X => self.ends[1].x - self.ends[0].x,
                    Axis::Y => self.ends[1].y - self.ends[0].y,
                };
                if width == 0.0 { 0.0 } else { delta / width }
            }
        };
        Ok(t.clamp(0.0, 1.0))
    }

    /// A rigorous parameter bracket; a guess or small residual alone is not one.
    fn bracket(&self, height: f64, math: &mut Work<'_>) -> Result<Interval> {
        math.blocks(1)?;
        if height <= self.bottom() {
            return Ok(Interval::exact(0.0));
        }
        if height >= self.top() {
            return Ok(Interval::exact(1.0));
        }
        let mut range = Interval { lo: 0.0, hi: 1.0 };
        let mut t = self.guess(Axis::Y, height, math)?;
        for _ in 0..8 {
            math.blocks(1)?;
            let y = self.coordinate(Axis::Y, t, math)?;
            if y.hi < height {
                range.lo = t;
            } else if y.lo > height {
                range.hi = t;
            }
            math.blocks(1)?;
            let [_, b, a] = self.y;
            let derivative = b.add(a.mul(Interval::exact(2.0)).mul(range));
            if derivative.lo > 0.0 {
                let residual = y.sub(Interval::exact(height)).radius();
                let radius = Interval::exact(residual)
                    .div(Interval::exact(derivative.lo))
                    .hi;
                range.lo = range.lo.max((t - radius).next_down()).max(0.0);
                range.hi = range.hi.min((t + radius).next_up()).min(1.0);
            }
            if !range.finite() || range.lo > range.hi {
                return Ok(unknown());
            }
            if range.hi - range.lo <= 16.0 * f64::EPSILON {
                break;
            }
            t = range.lo.midpoint(range.hi);
        }
        Ok(range)
    }

    fn x_range(&self, lo: f64, hi: f64, math: &mut Work<'_>) -> Result<Interval> {
        math.blocks(1)?;
        let a = self.bracket(lo, math)?;
        let b = self.bracket(hi, math)?;
        let left = self.coordinate(Axis::X, a.lo, math)?;
        let right = self.coordinate(Axis::X, b.hi, math)?;
        Ok(Interval {
            lo: left.lo.min(right.lo),
            hi: left.hi.max(right.hi),
        })
    }

    /// Directed right-side integral over one shared, literal source-Y slab.
    /// The caller proves this piece spans the whole slab and orders its X range.
    pub(crate) fn strip(
        &self,
        [lo, hi]: [f64; 2],
        shift: Interval,
        math: &mut Work<'_>,
    ) -> Result<Interval> {
        math.blocks(1)?;
        if !lo.is_finite() || !hi.is_finite() || lo >= hi || lo < self.bottom() || hi > self.top() {
            return Ok(unknown());
        }
        let a = if lo == self.bottom() {
            0.0
        } else {
            self.guess(Axis::Y, lo, math)?
        };
        let b = if hi == self.top() {
            1.0
        } else {
            self.guess(Axis::Y, hi, math)?
        };
        if !a.is_finite() || !b.is_finite() || a > b {
            return Ok(unknown());
        }
        let ya = self.coordinate(Axis::Y, a, math)?;
        let yb = self.coordinate(Axis::Y, b, math)?;
        math.blocks(1)?;
        let ey = Interval::exact(if lo == self.bottom() {
            0.0
        } else {
            ya.sub(Interval::exact(lo)).radius()
        })
        .add(Interval::exact(if hi == self.top() {
            0.0
        } else {
            yb.sub(Interval::exact(hi)).radius()
        }))
        .upper();
        let height = yb.sub(ya);
        let xa = self.coordinate(Axis::X, a, math)?.add(shift);
        let xb = self.coordinate(Axis::X, b, math)?.add(shift);
        let mut area = if xa.hi.max(xb.hi) <= 0.0 {
            height
        } else if xa.lo.min(xb.lo) >= 1.0 {
            Interval::exact(0.0)
        } else if self.ends[0].x == self.ends[1].x {
            let complement = Interval::exact(1.0).sub(self.x[0].add(shift));
            Interval {
                lo: complement.lo.clamp(0.0, 1.0),
                hi: complement.hi.clamp(0.0, 1.0),
            }
            .mul(height)
        } else if within_unit(xa, xb) {
            self.inside([a, b], shift, math)?
        } else {
            self.crossing([a, b], shift, math)?
        };
        math.blocks(1)?;
        area = area.widen(ey);
        if self.direction < 0 {
            area = Interval::exact(-1.0).mul(area);
        }
        Ok(area)
    }

    fn inside(&self, [a, b]: [f64; 2], shift: Interval, math: &mut Work<'_>) -> Result<Interval> {
        math.blocks(1)?;
        let ya = self.coordinate(Axis::Y, a, math)?;
        let yb = self.coordinate(Axis::Y, b, math)?;
        let ha = self.integral(a, math)?;
        let hb = self.integral(b, math)?;
        Ok(Interval::exact(1.0)
            .sub(self.x[0].add(shift))
            .mul(yb.sub(ya))
            .sub(hb.sub(ha)))
    }

    fn crossing(&self, times: [f64; 2], shift: Interval, math: &mut Work<'_>) -> Result<Interval> {
        math.blocks(2)?;
        let [a, b] = times;
        let increasing = self.ends[0].x < self.ends[1].x;
        let (first, second) = if increasing { (0.0, 1.0) } else { (1.0, 0.0) };
        let xa = self.coordinate(Axis::X, a, math)?.add(shift);
        let xb = self.coordinate(Axis::X, b, math)?.add(shift);
        let [absent_first, absent_second] = self.absent_x_cuts(xa, xb);
        let u = if absent_first {
            a
        } else {
            self.guess(Axis::X, first - shift.lo, math)?.clamp(a, b)
        };
        let v = if absent_second {
            b
        } else {
            self.guess(Axis::X, second - shift.hi, math)?.clamp(a, b)
        };
        if !u.is_finite() || !v.is_finite() || u > v {
            return Ok(unknown());
        }
        let yu = self.coordinate(Axis::Y, u, math)?;
        let yv = self.coordinate(Axis::Y, v, math)?;
        let ya = self.coordinate(Axis::Y, a, math)?;
        let yb = self.coordinate(Axis::Y, b, math)?;
        let hu = self.integral(u, math)?;
        let hv = self.integral(v, math)?;
        let xu = self.coordinate(Axis::X, u, math)?.add(shift);
        let xv = self.coordinate(Axis::X, v, math)?.add(shift);
        let inside = Interval::exact(1.0)
            .sub(self.x[0].add(shift))
            .mul(yv.sub(yu))
            .sub(hv.sub(hu));
        let full = if increasing { yu.sub(ya) } else { yb.sub(yv) };
        let residual = Interval::exact(if absent_first {
            0.0
        } else {
            xu.sub(Interval::exact(first)).radius()
        })
        .add(Interval::exact(if absent_second {
            0.0
        } else {
            xv.sub(Interval::exact(second)).radius()
        }))
        .upper();
        let ex = Interval::exact(yb.sub(ya).hi.max(0.0))
            .mul(Interval::exact(residual))
            .hi;
        Ok(full.add(inside).add(Interval { lo: -ex, hi: ex }))
    }

    fn absent_x_cuts(&self, a: Interval, b: Interval) -> [bool; 2] {
        if !a.finite() || !b.finite() {
            [false, false]
        } else if self.ends[0].x < self.ends[1].x {
            [a.lo >= 0.0, b.hi <= 1.0]
        } else {
            [a.hi <= 1.0, b.lo >= 0.0]
        }
    }
}

/// Endpoint enclosures, on the same monotone shadow, prove absence of X cuts.
fn within_unit(a: Interval, b: Interval) -> bool {
    a.lo.min(b.lo) >= 0.0 && a.hi.max(b.hi) <= 1.0
}

fn unknown() -> Interval {
    Interval {
        lo: f64::NEG_INFINITY,
        hi: f64::INFINITY,
    }
}

/// Temporary source-relative facts, dropped before mandatory preparation resumes.
#[derive(Debug)]
pub(crate) struct Metadata {
    pieces: Vec<Polynomial>,
    points: Vec<Point>,
    heights: Vec<f64>,
    error: f64,
    clamp_error: f64,
}

/// An endpoint-preserving shadow, with its separate unit-pixel homotopy fee.
fn monotone_shadow(piece: Piece, math: &mut Work<'_>) -> Result<(Piece, f64)> {
    math.blocks(1)?;
    let Piece::Curve(q) = piece else {
        return Ok((piece, 0.0));
    };
    let [a, b, c] = q.points;
    if ![a, b, c]
        .into_iter()
        .all(|p| p.x.is_finite() && p.y.is_finite())
    {
        return Ok((piece, f64::INFINITY));
    }
    let control = Point::new(
        b.x.clamp(a.x.min(c.x), a.x.max(c.x)),
        b.y.clamp(a.y.min(c.y), a.y.max(c.y)),
    );
    if control == b {
        return Ok((piece, 0.0));
    }
    let dx = Interval::exact(b.x).sub(Interval::exact(control.x));
    let dy = Interval::exact(b.y).sub(Interval::exact(control.y));
    let error = Interval::exact(2.0)
        .mul(Interval::exact(dx.radius()).add(Interval::exact(dy.radius())))
        .upper();
    Ok((
        Piece::Curve(Quadratic {
            points: [a, control, c],
        }),
        error,
    ))
}

/// Error-free stored-coordinate subtraction, retaining its exact tail.
pub(crate) fn shift(a: f64, b: f64) -> Interval {
    let high = a - b;
    let back = high - a;
    let tail = (a - (high - back)) + (-b - back);
    Interval::exact(high).add(Interval::exact(tail))
}

fn mapped_interval(point: Point, linear: Transform, origin: Point) -> [Interval; 2] {
    let x = Interval::exact(point.x);
    let y = Interval::exact(point.y);
    [
        x.mul(Interval::exact(linear.a))
            .add(y.mul(Interval::exact(linear.c)))
            .add(Interval::exact(linear.e))
            .add(Interval::exact(origin.x)),
        x.mul(Interval::exact(linear.b))
            .add(y.mul(Interval::exact(linear.d)))
            .add(Interval::exact(linear.f))
            .add(Interval::exact(origin.y)),
    ]
}

fn nominal(point: Point, origin: Point) -> [Interval; 2] {
    [
        Interval::exact(point.x).add(Interval::exact(origin.x)),
        Interval::exact(point.y).add(Interval::exact(origin.y)),
    ]
}

fn displaced<const N: usize>(
    left: [[Interval; 2]; N],
    right: [[Interval; 2]; N],
    degree: f64,
) -> f64 {
    let mut e = [0.0_f64; 2];
    for (a, b) in left.into_iter().zip(right) {
        for axis in 0..2 {
            e[axis] = e[axis].max(a[axis].sub(b[axis]).radius());
        }
    }
    Interval::exact(degree)
        .mul(Interval::exact(e[0]).add(Interval::exact(e[1])))
        .hi
}

fn elevated(q: [[Interval; 2]; 3]) -> [[Interval; 2]; 4] {
    let mix = |a: [Interval; 2], b: [Interval; 2]| {
        std::array::from_fn(|i| {
            a[i].add(Interval::exact(2.0).mul(b[i]))
                .div(Interval::exact(3.0))
        })
    };
    [q[0], mix(q[0], q[1]), mix(q[2], q[1]), q[2]]
}

fn restricted(q: Quadratic, a: f64, b: f64) -> [[Interval; 2]; 3] {
    std::array::from_fn(|index| {
        std::array::from_fn(|axis| {
            let c = coefficients(q.points.map(|p| if axis == 0 { p.x } else { p.y }));
            match index {
                0 => value(c, a),
                2 => value(c, b),
                _ => value(c, a).add(
                    Interval::exact(b).sub(Interval::exact(a)).mul(
                        c[1].add(c[2].mul(Interval::exact(2.0 * a)))
                            .mul(Interval::exact(0.5)),
                    ),
                ),
            }
        })
    })
}

fn same_piece(a: Piece, b: Piece) -> bool {
    let point =
        |a: Point, b: Point| [a.x.to_bits(), a.y.to_bits()] == [b.x.to_bits(), b.y.to_bits()];
    match (a, b) {
        (Piece::Line(a), Piece::Line(b)) => point(a.a, b.a) && point(a.b, b.b),
        (Piece::Curve(a), Piece::Curve(b)) => {
            a.points.into_iter().zip(b.points).all(|(a, b)| point(a, b))
        }
        _ => false,
    }
}

fn split_error(
    q: Quadratic,
    source: [Point; 4],
    limits: &Limits,
    bytes: &mut Meter,
    math: &mut Work<'_>,
) -> Result<Option<f64>> {
    math.blocks(2)?;
    let [_, v, a] = q.coefficients();
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
    let before = bytes.used();
    let mut scratch = Spline::default();
    bytes.reserve_capacity(&mut scratch.pieces, 3)?;
    math.blocks(4)?;
    let mut builder = Builder {
        spline: scratch,
        limits,
        bytes,
        point_contours: PointContours::Keep,
    };
    builder.cubic(source)?;
    let actual = builder.spline;
    let mut next = actual.pieces.iter();
    let mut error = Interval::exact(0.0);
    for pair in events.windows(2) {
        if pair[0] == pair[1] {
            continue;
        }
        math.blocks(24)?;
        let mut leaf = q.interval(pair[0], pair[1]);
        leaf.points[0] = if pair[0] == 0.0 {
            q.points[0]
        } else {
            q.at(pair[0])
        };
        leaf.points[2] = if pair[1] == 1.0 {
            q.points[2]
        } else {
            q.at(pair[1])
        };
        let emitted = if cross(v, a) == 0.0 || leaf.points[0].y == leaf.points[2].y {
            Piece::Line(Line {
                a: leaf.points[0],
                b: leaf.points[2],
            })
        } else {
            Piece::Curve(leaf)
        };
        if !matches!(emitted,Piece::Line(Line{a,b}) if a==b) {
            if !next.next().is_some_and(|&p| same_piece(p, emitted)) {
                return Ok(None);
            }
        }
        // R -> L belongs to the complete paired closed chains, including
        // these shared literal endpoint overrides. It does not compare an
        // open quadratic against the chord's unrelated midpoint parameter.
        let target = leaf.points.map(|p| nominal(p, Point::default()));
        let split = displaced(restricted(q, pair[0], pair[1]), target, 2.0);
        error = error.add(Interval::exact(split));
        if matches!(emitted, Piece::Line(_)) {
            // L -> chord has exactly identical endpoint bits, including an
            // omitted zero chord. The closed parabolic-segment loop bounds
            // its filled-set change even in arbitrary external winding.
            let chord = chord_area(leaf.points, math)?;
            error = error.add(Interval::exact(chord));
        }
    }
    if next.next().is_some() {
        return Ok(None);
    }
    drop(actual);
    bytes.refund(bytes.used().saturating_sub(before));
    Ok(Some(error.hi))
}

fn chord_area([a, b, c]: [Point; 3], math: &mut Work<'_>) -> Result<f64> {
    math.blocks(1)?;
    let bx = Interval::exact(b.x).sub(Interval::exact(a.x));
    let by = Interval::exact(b.y).sub(Interval::exact(a.y));
    let cx = Interval::exact(c.x).sub(Interval::exact(a.x));
    let cy = Interval::exact(c.y).sub(Interval::exact(a.y));
    let determinant = bx.mul(cy).sub(by.mul(cx));
    Ok(Interval::exact(determinant.radius())
        .div(Interval::exact(3.0))
        .upper())
}

fn source_error(
    path: &Path,
    linear: Transform,
    origin: Point,
    limits: &Limits,
    bytes: &mut Meter,
    math: &mut Work<'_>,
) -> Result<Option<f64>> {
    let (mut current, mut start) = (None, Point::default());
    let mut error = Interval::exact(0.0);
    let line = |a: Point, b: Point| {
        displaced(
            [
                mapped_interval(a, linear, origin),
                mapped_interval(b, linear, origin),
            ],
            [nominal(a * linear, origin), nominal(b * linear, origin)],
            1.0,
        )
    };
    for segment in path.segments() {
        math.blocks(32)?;
        match *segment {
            Segment::MoveTo(p) => {
                if let Some(a) = current {
                    let fee = line(a, start);
                    error = error.add(Interval::exact(fee));
                }
                start = p;
                current = Some(p);
            }
            Segment::LineTo(p) => {
                if let Some(a) = current {
                    let fee = line(a, p);
                    error = error.add(Interval::exact(fee));
                }
                current = Some(p);
            }
            Segment::Close => {
                if let Some(a) = current {
                    let fee = line(a, start);
                    error = error.add(Interval::exact(fee));
                }
                current = Some(start);
            }
            Segment::CubicTo { c1, c2, end } => {
                let Some(p) = current else {
                    return Ok(None);
                };
                let source = [p, c1, c2, end];
                let [a, b, c, d] = source.map(|p| p * linear);
                let q = Quadratic {
                    points: [
                        a,
                        add(lerp(a, d, 0.5), scale(add(sub(b, a), sub(c, d)), 0.75)),
                        d,
                    ],
                };
                let delta = displaced(
                    source.map(|p| mapped_interval(p, linear, origin)),
                    elevated(q.points.map(|p| nominal(p, origin))),
                    3.0,
                );
                let Some(split) = split_error(q, [a, b, c, d], limits, bytes, math)? else {
                    return Ok(None);
                };
                error = error
                    .add(Interval::exact(delta))
                    .add(Interval::exact(split));
                current = Some(end);
            }
        }
    }
    if let Some(a) = current {
        math.blocks(32)?;
        let fee = line(a, start);
        error = error.add(Interval::exact(fee));
    }
    Ok(Some(error.hi))
}

/// Builds temporary original-curve evidence; unsupported sources decline.
pub(crate) fn prepare(
    path: &Path,
    linear: Transform,
    expected_points: &[Point],
    limits: &Limits,
    bytes: &mut Meter,
    math: &mut Work<'_>,
) -> Result<Option<Metadata>> {
    // Three grammar/reduction/Builder source passes, then the complete
    // checked event bounds: 3*n+1 pieces, n+1 starts and n point cells.
    // The five fixed blocks cover initial empty-owner reservations/events.
    math.work.charge_product(path.segments().len(), 8)?;
    math.blocks(5)?;
    let Some(spline) = proof_spline(path, linear, limits, bytes)? else {
        return Ok(None);
    };
    math.blocks(spline.points.len())?;
    if spline.points.len() != expected_points.len()
        || !spline
            .points
            .iter()
            .zip(expected_points)
            .all(|(a, b)| a.x.to_bits() == b.x.to_bits() && a.y.to_bits() == b.y.to_bits())
    {
        return Ok(None);
    }
    let Some(error) = source_error(path, linear, Point::default(), limits, bytes, math)? else {
        return Ok(None);
    };
    if !error.is_finite() {
        return Ok(None);
    }
    let mut pieces = Vec::new();
    math.blocks(1)?;
    bytes.reserve_capacity(&mut pieces, spline.pieces.len())?;
    let mut clamp_error = Interval::exact(0.0);
    for &piece in &spline.pieces {
        math.blocks(2)?;
        let (piece, error) = monotone_shadow(piece, math)?;
        if error > 0.0 {
            math.blocks(1)?;
            clamp_error = clamp_error.add(Interval::exact(error));
        }
        let Some(p) = Polynomial::new(piece, math)? else {
            return Ok(None);
        };
        pieces.push(p);
    }
    if !clamp_error.finite() {
        return Ok(None);
    }
    let mut heights = Vec::new();
    let Some(count) = pieces.len().checked_mul(2) else {
        return Ok(None);
    };
    math.blocks(1)?;
    bytes.reserve_capacity(&mut heights, count)?;
    math.work.charge_product(pieces.len(), 3)?;
    for p in &pieces {
        if p.bottom() != p.top() {
            heights.extend([p.bottom(), p.top()]);
        }
    }
    sort_heights(&mut heights, math)?;
    math.work.charge_product(heights.len(), 2)?;
    heights.dedup();
    let Spline {
        pieces: old,
        starts,
        points,
    } = spline;
    let Some(released) = old
        .capacity()
        .checked_mul(size_of::<Piece>())
        .and_then(|pieces| {
            starts
                .capacity()
                .checked_mul(size_of::<usize>())
                .and_then(|starts| pieces.checked_add(starts))
        })
    else {
        return Ok(None);
    };
    drop(old);
    drop(starts);
    bytes.refund(released);
    Ok(Some(Metadata {
        pieces,
        points,
        heights,
        error,
        clamp_error: clamp_error.upper(),
    }))
}

fn proof_spline(
    path: &Path,
    linear: Transform,
    limits: &Limits,
    bytes: &mut Meter,
) -> Result<Option<Spline>> {
    if path.segments().len() > limits.path_segments
        || !quadratic_outline(path, linear)
        || !reducible_with_rounding(path, linear)
    {
        return Ok(None);
    }
    let count = path.segments().len();
    let Some(pieces) = count.checked_mul(3).and_then(|n| n.checked_add(1)) else {
        return Ok(None);
    };
    let Some(starts) = count.checked_add(1) else {
        return Ok(None);
    };
    let mut spline = Spline::default();
    bytes.reserve_capacity(&mut spline.pieces, pieces)?;
    bytes.reserve_capacity(&mut spline.starts, starts)?;
    bytes.reserve_capacity(&mut spline.points, count)?;
    Builder {
        spline,
        limits,
        bytes,
        point_contours: PointContours::Collect,
    }
    .path(path, linear)
}

fn sort_heights(values: &mut [f64], math: &mut Work<'_>) -> Result<()> {
    for root in (0..values.len() / 2).rev() {
        sift(values, root, math)?;
    }
    for end in (1..values.len()).rev() {
        math.blocks(2)?;
        values.swap(0, end);
        if let Some(heap) = values.get_mut(..end) {
            sift(heap, 0, math)?;
        }
    }
    Ok(())
}

fn sift(values: &mut [f64], mut root: usize, math: &mut Work<'_>) -> Result<()> {
    loop {
        math.blocks(1)?;
        let Some(mut child) = root.checked_mul(2).and_then(|n| n.checked_add(1)) else {
            return Ok(());
        };
        let Some(&left) = values.get(child) else {
            return Ok(());
        };
        if let Some(&right) = values.get(child + 1) {
            math.blocks(1)?;
            if left.total_cmp(&right).is_lt() {
                child += 1;
            }
        }
        let (Some(&a), Some(&b)) = (values.get(root), values.get(child)) else {
            return Ok(());
        };
        math.blocks(1)?;
        if !a.total_cmp(&b).is_lt() {
            return Ok(());
        }
        math.blocks(2)?;
        values.swap(root, child);
        root = child;
    }
}

const MAX_NODES: usize = 64;
const MAX_TOUCHES: usize = 16;
const MAX_DEPTH: usize = 4;
const RECTANGLE_TARGET: f64 = 1.0 / 4096.0;

#[derive(Clone, Copy)]
struct Query {
    origin: Point,
    x: u32,
    y: u32,
}
#[derive(Clone, Copy)]
struct Window {
    lo: f64,
    hi: f64,
    left: f64,
    right: f64,
    shift: Interval,
    vertical_error: f64,
    points: i64,
}
#[derive(Clone, Copy)]
struct Crossing {
    lo: f64,
    hi: f64,
    delta: i64,
    negative: i64,
    positive: i64,
    count: usize,
    index: usize,
}
#[derive(Clone, Copy)]
struct Integral {
    area: Interval,
    rectangle_error: f64,
}
#[derive(Clone, Copy)]
struct Refinement {
    depth: usize,
    reserved: usize,
}
struct Nodes {
    used: usize,
}
#[derive(Debug, Clone, Copy)]
enum Failure {
    Geometry,
    Nonfinite,
    Capacity,
    Incompatible,
}

impl Metadata {
    /// A successful enclosure concerns the original source in this unit pixel.
    pub(crate) fn unit(
        &self,
        origin: Point,
        pixel: [u32; 2],
        math: &mut Work<'_>,
    ) -> Result<Option<Interval>> {
        let q = Query {
            origin,
            x: pixel[0],
            y: pixel[1],
        };
        match unit_interval(self, q, math)? {
            Ok(area) if area.finite() => Ok(Some(area)),
            _ => Ok(None),
        }
    }

    /// The shadow and exact canonical point cells must fit the identical clip.
    pub(crate) fn inside_rect(
        &self,
        origin: Point,
        rect: Bounds,
        math: &mut Work<'_>,
    ) -> Result<bool> {
        let contains = |p: Point| {
            let x = Interval::exact(p.x).add(Interval::exact(origin.x));
            let y = Interval::exact(p.y).add(Interval::exact(origin.y));
            x.finite()
                && y.finite()
                && rect.left <= x.lo
                && x.hi <= rect.right
                && rect.top <= y.lo
                && y.hi <= rect.bottom
        };
        for p in &self.pieces {
            math.blocks(1)?;
            if !p.ends.into_iter().all(contains) {
                return Ok(false);
            }
        }
        for &p in &self.points {
            math.blocks(1)?;
            let [a, _, b, _] = crate::band::edges::Polygon::cell_corners(p + origin);
            if ![a.x, a.y, b.x, b.y].into_iter().all(f64::is_finite)
                || a.x < rect.left
                || b.x > rect.right
                || a.y < rect.top
                || b.y > rect.bottom
            {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

fn sort_crossings(values: &mut [Crossing], math: &mut Work<'_>) -> Result<()> {
    for index in 1..values.len() {
        let mut at = index;
        while at > 0 {
            math.blocks(1)?;
            let (Some(a), Some(b)) = (values.get(at - 1), values.get(at)) else {
                return Ok(());
            };
            if !a.lo.total_cmp(&b.lo).is_gt() {
                break;
            }
            math.blocks(2)?;
            values.swap(at - 1, at);
            at -= 1;
        }
    }
    Ok(())
}

fn nominal_window(meta: &Metadata, q: Query, math: &mut Work<'_>) -> Result<Option<Window>> {
    math.blocks(2)?;
    math.work.charge_product(meta.points.len(), 8)?;
    let count = meta
        .points
        .iter()
        .filter(|p| {
            (p.x + q.origin.x).floor() == f64::from(q.x)
                && (p.y + q.origin.y).floor() == f64::from(q.y)
        })
        .count();
    let points = match i64::try_from(count) {
        Ok(n) => -n,
        Err(_) => return Ok(None),
    };
    let lo = f64::from(q.y) - q.origin.y;
    let hi = lo + 1.0;
    let exact_y = shift(f64::from(q.y), q.origin.y);
    let one = Interval::exact(1.0);
    let vertical_error = Interval::exact(exact_y.sub(Interval::exact(lo)).radius())
        .add(Interval::exact(
            exact_y.add(one).sub(Interval::exact(hi)).radius(),
        ))
        .upper();
    let exact_x = shift(f64::from(q.x), q.origin.x);
    let right = exact_x.add(one).upper();
    let shift = shift(q.origin.x, f64::from(q.x));
    if !lo.is_finite()
        || !hi.is_finite()
        || lo >= hi
        || !vertical_error.is_finite()
        || !exact_x.finite()
        || !right.is_finite()
        || !shift.finite()
    {
        return Ok(None);
    }
    Ok(Some(Window {
        lo,
        hi,
        left: exact_x.lower(),
        right,
        shift,
        vertical_error,
        points,
    }))
}

fn slab_integral(
    meta: &Metadata,
    cuts: [f64; 2],
    window: Window,
    math: &mut Work<'_>,
) -> Result<std::result::Result<Integral, Failure>> {
    let mut spans = [Crossing {
        lo: 0.0,
        hi: 0.0,
        delta: 0,
        negative: 0,
        positive: 0,
        count: 0,
        index: 0,
    }; MAX_TOUCHES];
    let mut n = 0;
    let mut winding = window.points;
    let mut total = 0_i64;
    for (index, piece) in meta.pieces.iter().enumerate() {
        math.blocks(1)?;
        math.blocks(1)?;
        if piece.top() <= cuts[0] || piece.bottom() >= cuts[1] {
            continue;
        }
        if piece.bottom() > cuts[0] || piece.top() < cuts[1] {
            return Ok(Err(Failure::Geometry));
        }
        total += piece.direction;
        let range = piece.x_range(cuts[0], cuts[1], math)?;
        if !range.finite() {
            return Ok(Err(Failure::Nonfinite));
        }
        if range.upper() < window.left {
            winding += piece.direction;
        } else if range.lower() <= window.right {
            if n == MAX_TOUCHES {
                return Ok(Err(Failure::Capacity));
            }
            spans[n] = Crossing {
                lo: range.lower(),
                hi: range.upper(),
                delta: piece.direction,
                negative: piece.direction.min(0),
                positive: piece.direction.max(0),
                count: 1,
                index,
            };
            n += 1;
        }
    }
    if total != 0 {
        return Ok(Err(Failure::Geometry));
    }
    sort_crossings(&mut spans[..n], math)?;
    math.blocks(n)?;
    let mut merged = 0;
    for i in 0..n {
        let span = spans[i];
        if merged > 0 && span.lo <= spans[merged - 1].hi {
            let last = &mut spans[merged - 1];
            last.hi = last.hi.max(span.hi);
            last.delta += span.delta;
            last.negative += span.negative;
            last.positive += span.positive;
            last.count += 1;
        } else {
            spans[merged] = span;
            merged += 1;
        }
    }
    integrate_crossings(meta, &spans[..merged], cuts, window, winding, math)
}

fn integrate_crossings(
    meta: &Metadata,
    spans: &[Crossing],
    cuts: [f64; 2],
    window: Window,
    mut winding: i64,
    math: &mut Work<'_>,
) -> Result<std::result::Result<Integral, Failure>> {
    math.blocks(1)?;
    let mut rectangle_error = Interval::exact(0.0);
    let mut area = if winding == 0 {
        Interval::exact(0.0)
    } else {
        Interval::exact(cuts[1]).sub(Interval::exact(cuts[0]))
    };
    for c in spans {
        math.blocks(1)?;
        let after = winding + c.delta;
        if c.count != 1 {
            let inactive = winding + c.negative > 0 || winding + c.positive < 0;
            // Every possible internal prefix stays nonzero, so this whole
            // component leaves the covered indicator unchanged. Its signed
            // net winding still advances below for every following boundary.
            if !inactive {
                let part = component_rectangle(c, cuts, window, winding, math)?;
                if !part.area.finite() {
                    return Ok(Err(Failure::Nonfinite));
                }
                math.blocks(1)?;
                area = area.add(part.area);
                rectangle_error = rectangle_error.add(Interval::exact(part.rectangle_error));
            }
        } else {
            let change = i64::from(after != 0) - i64::from(winding != 0);
            if change != 0 {
                let part = meta.pieces[c.index].strip(cuts, window.shift, math)?;
                if !part.finite() {
                    return Ok(Err(Failure::Nonfinite));
                }
                // strip is already directed: change/direction == change*direction.
                let coefficient = change * c.delta;
                math.blocks(1)?;
                let factor = match coefficient {
                    -1 => -1.0,
                    1 => 1.0,
                    _ => return Ok(Err(Failure::Geometry)),
                };
                area = area.add(part.mul(Interval::exact(factor)));
            }
        }
        winding = after;
    }
    Ok(Ok(Integral {
        area,
        rectangle_error: rectangle_error.upper(),
    }))
}

/// One nominal step plus a one-sided covered-set error on its whole envelope.
fn component_rectangle(
    c: &Crossing,
    cuts: [f64; 2],
    window: Window,
    before: i64,
    math: &mut Work<'_>,
) -> Result<Integral> {
    math.blocks(1)?;
    let after = before + c.delta;
    let change = i32::from(after != 0) - i32::from(before != 0);
    let height = Interval::exact(cuts[1]).sub(Interval::exact(cuts[0]));
    let width = Interval::exact(c.hi.min(window.right)).sub(Interval::exact(c.lo.max(window.left)));
    if !height.finite() || !width.finite() {
        return Ok(Integral {
            area: Interval::bounds(f64::NAN, f64::NAN),
            rectangle_error: 0.0,
        });
    }
    // The actual physical-X interval has exact width one even if its enclosing
    // source-space endpoints include subtraction tails.
    let overlap = width.upper().clamp(0.0, 1.0);
    let error = Interval::exact(overlap).mul(height).upper();
    let complement = Interval::exact(1.0).sub(Interval::exact(c.lo).add(window.shift));
    let weight = Interval::bounds(
        complement.lower().clamp(0.0, 1.0),
        complement.upper().clamp(0.0, 1.0),
    );
    let nominal = weight.mul(height).mul(Interval::exact(f64::from(change)));
    // Inside the envelope the nominal state is exactly the AFTER state.
    let uncertainty = if after != 0 {
        Interval::bounds(-error, 0.0)
    } else {
        Interval::bounds(0.0, error)
    };
    Ok(Integral {
        area: nominal.add(uncertainty),
        rectangle_error: error,
    })
}

/// Every attempted parent or child consumes one of the same 64 nodes.
fn refined_slab(
    meta: &Metadata,
    cuts: [f64; 2],
    window: Window,
    step: Refinement,
    nodes: &mut Nodes,
    math: &mut Work<'_>,
) -> Result<std::result::Result<Interval, Failure>> {
    if nodes.used + step.reserved >= MAX_NODES {
        return Ok(Err(Failure::Capacity));
    }
    nodes.used += 1;
    math.blocks(2)?;
    let parent = match slab_integral(meta, cuts, window, math)? {
        Ok(parent) => parent,
        Err(reason) => return Ok(Err(reason)),
    };
    if parent.rectangle_error <= RECTANGLE_TARGET {
        return Ok(Ok(parent.area));
    }
    if step.depth == MAX_DEPTH {
        return Ok(Ok(parent.area));
    }
    let mid = cuts[0].midpoint(cuts[1]);
    if !(cuts[0] < mid && mid < cuts[1]) {
        return Ok(Ok(parent.area));
    }
    if nodes.used + step.reserved + 2 > MAX_NODES {
        return Ok(Ok(parent.area));
    }
    let left = match refined_slab(
        meta,
        [cuts[0], mid],
        window,
        Refinement {
            depth: step.depth + 1,
            reserved: step.reserved + 1,
        },
        nodes,
        math,
    )? {
        Ok(left) => left,
        Err(reason @ (Failure::Capacity | Failure::Incompatible)) => return Ok(Err(reason)),
        Err(_) => {
            return Ok(Ok(parent.area));
        }
    };
    let right = match refined_slab(
        meta,
        [mid, cuts[1]],
        window,
        Refinement {
            depth: step.depth + 1,
            reserved: step.reserved,
        },
        nodes,
        math,
    )? {
        Ok(right) => right,
        Err(reason @ (Failure::Capacity | Failure::Incompatible)) => return Ok(Err(reason)),
        Err(_) => {
            return Ok(Ok(parent.area));
        }
    };
    math.blocks(1)?;
    let children = left.add(right);
    if !children.finite() {
        return Ok(Ok(parent.area));
    }
    let area = Interval::bounds(
        parent.area.lower().max(children.lower()),
        parent.area.upper().min(children.upper()),
    );
    if !area.finite() {
        return Ok(Err(Failure::Incompatible));
    }
    Ok(Ok(area))
}

fn unit_interval(
    meta: &Metadata,
    q: Query,
    math: &mut Work<'_>,
) -> Result<std::result::Result<Interval, Failure>> {
    let mut nodes = Nodes { used: 0 };
    let Some(window) = nominal_window(meta, q, math)? else {
        return Ok(Err(Failure::Nonfinite));
    };
    math.blocks(meta.heights.len().saturating_add(1))?;
    let required = meta
        .heights
        .iter()
        .filter(|&&y| window.lo < y && y < window.hi)
        .count()
        + 1;
    if required > MAX_NODES {
        return Ok(Err(Failure::Capacity));
    }
    let mut lo = window.lo;
    let mut area = Interval::exact(0.0);
    let mut touched = 0;
    // Shared literal cuts include both point-only exterior slabs.
    for &hi in meta.heights.iter().chain(std::iter::once(&window.hi)) {
        math.blocks(1)?;
        if hi <= lo || hi > window.hi {
            continue;
        }
        touched += 1;
        let part = match refined_slab(
            meta,
            [lo, hi],
            window,
            Refinement {
                depth: 0,
                reserved: required - touched,
            },
            &mut nodes,
            math,
        )? {
            Ok(part) => part,
            Err(reason) => return Ok(Err(reason)),
        };
        math.blocks(1)?;
        area = area.add(part);
        lo = hi;
    }
    if lo != window.hi {
        return Ok(Err(Failure::Geometry));
    }
    math.blocks(1)?;
    let area = area
        .widen(meta.error)
        .widen(meta.clamp_error)
        .widen(window.vertical_error);
    Ok(Ok(area))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PathBuilder;

    #[derive(Clone, Copy)]
    enum Direction {
        Forward,
        Reverse,
    }

    fn outline(direction: Direction, point: Option<Point>) -> Path {
        let a = Point::new(1.0 / 8.0, 0.0);
        let b = Point::new(1.0 / 2.0, 1.0 / 2.0);
        let c = Point::new(3.0 / 4.0, 1.0);
        let first = a + (b - a) * (2.0 / 3.0);
        let second = c + (b - c) * (2.0 / 3.0);
        let mut path = PathBuilder::new();
        match direction {
            Direction::Forward => {
                path.move_to(a);
                path.cubic_to(first, second, c).unwrap();
                path.line_to(Point::new(1.0, 1.0)).unwrap();
                path.line_to(Point::new(1.0, 0.0)).unwrap();
            }
            Direction::Reverse => {
                path.move_to(a);
                path.line_to(Point::new(1.0, 0.0)).unwrap();
                path.line_to(Point::new(1.0, 1.0)).unwrap();
                path.line_to(c).unwrap();
                path.cubic_to(second, first, a).unwrap();
            }
        }
        path.close();
        if let Some(point) = point {
            path.move_to(point);
            path.close();
        }
        path.finish()
    }

    #[test]
    fn rational_quadratic_and_canonical_point_cancellation_have_narrow_bounds() {
        let limits = Limits::default();
        for direction in [Direction::Forward, Direction::Reverse] {
            for point in [None, Some(Point::new(0.25, 0.25))] {
                let path = outline(direction, point);
                let expected_points: Vec<_> = point.into_iter().collect();
                let mut work = Meter::new(limits.raster_work, "test work");
                let mut bytes = Meter::new(limits.render_bytes, "test bytes");
                let mut math = Work::new(&mut work);
                let meta = prepare(
                    &path,
                    Transform::IDENTITY,
                    &expected_points,
                    &limits,
                    &mut bytes,
                    &mut math,
                )
                .unwrap()
                .unwrap();
                let area = meta
                    .unit(Point::default(), [0, 0], &mut math)
                    .unwrap()
                    .unwrap();
                // Integral X dy is (1/8 + 1/2 + 3/4)/3 = 11/24.
                let exact = match (direction, point) {
                    (_, None) => 13.0 / 24.0,
                    (Direction::Forward, Some(_)) => 11.0 / 24.0,
                    (Direction::Reverse, Some(_)) => 1.0,
                };
                assert!(area.finite());
                assert!(area.upper() - area.lower() < 1e-8);
                assert!(area.lower() <= exact && exact <= area.upper());
            }
        }
    }

    #[test]
    fn rectangle_gate_contains_placed_shadow_and_literal_floor_cell() {
        let limits = Limits::default();
        let point = Point::new(-0.25, 0.25);
        let path = outline(Direction::Forward, Some(point));
        let mut work = Meter::new(limits.raster_work, "test work");
        let mut bytes = Meter::new(limits.render_bytes, "test bytes");
        let mut math = Work::new(&mut work);
        let meta = prepare(
            &path,
            Transform::IDENTITY,
            &[point],
            &limits,
            &mut bytes,
            &mut math,
        )
        .unwrap()
        .unwrap();
        let origin = Point::new(4.0, 0.5);
        let rect = Bounds {
            left: 2.0,
            top: -1.0,
            right: 6.0,
            bottom: 3.0,
        };
        assert!(meta.inside_rect(origin, rect, &mut math).unwrap());
        // The body is right of 4, but floor(-.25+4) puts its point cell at 3.
        assert!(
            !meta
                .inside_rect(origin, Bounds { left: 4.0, ..rect }, &mut math)
                .unwrap()
        );
        assert!(
            !meta
                .inside_rect(origin, Bounds { right: 4.5, ..rect }, &mut math)
                .unwrap()
        );
    }

    #[test]
    fn optional_setup_keeps_current_path_and_child_meter_limits() {
        let path = outline(Direction::Forward, None);
        let limits = Limits {
            path_segments: 2,
            ..Limits::default()
        };
        let mut work = Meter::new(limits.raster_work, "test work");
        let mut bytes = Meter::new(limits.render_bytes, "test bytes");
        assert!(
            prepare(
                &path,
                Transform::IDENTITY,
                &[],
                &limits,
                &mut bytes,
                &mut Work::new(&mut work)
            )
            .unwrap()
            .is_none()
        );
        let limits = Limits::default();
        let mut work = Meter::new(0, "test work");
        let error = prepare(
            &path,
            Transform::IDENTITY,
            &[],
            &limits,
            &mut bytes,
            &mut Work::new(&mut work),
        )
        .unwrap_err();
        assert_eq!(error.kind, pagura_core::ErrorKind::Limit);
        assert_eq!(work.used(), 0);
        let mut work = Meter::new(limits.raster_work, "test work");
        let mut bytes = Meter::new(0, "test bytes");
        let error = prepare(
            &path,
            Transform::IDENTITY,
            &[],
            &limits,
            &mut bytes,
            &mut Work::new(&mut work),
        )
        .unwrap_err();
        assert_eq!(error.kind, pagura_core::ErrorKind::Limit);
    }
}
