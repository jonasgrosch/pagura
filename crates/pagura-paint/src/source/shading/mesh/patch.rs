//! Bicubic tensor patches, including folded-surface precedence
//! (8.7.4.5.7–8.7.4.5.8).
//!
//! A point's parameters are found by Newton's method where a positive-definite
//! symmetric transformed Jacobian certifies that the patch is injective, and
//! otherwise by convex-hull subdivision that isolates the inverse images,
//! preferring the largest `v`, then the largest `u` (the surface drawn last).
//!
//! Subdivision stops at parameter boxes narrower than 2⁻²⁴, whose roots are
//! indistinguishable at component precision: a box whose root Newton's method
//! cannot resolve there yields its centre. A sample visits at most
//! [`MAX_WORK`] boxes; past that, it keeps the best root found so far. Both
//! are defined results where evaluation used to fail.

use super::super::Input;
use super::Bounds;
use pagura_core::{Meter, Result};
use pagura_raster::Point;

/// The most subdivision boxes one sample of one patch visits.
pub(super) const MAX_WORK: usize = 4096;

#[derive(Debug, Clone)]
pub(super) struct Patch {
    /// `p_ij` at `4 · j + i`, `i` indexing `u`: the transpose of the
    /// scene's layout, as the evaluation reads it.
    points: [Point; 16],
    /// Corner values at `p00`, `p03`, `p33`, `p30`, `values` each.
    colors: Box<[f64]>,
    regular: bool,
    bounds: Bounds,
}

/// The state of one sample's subdivision.
struct Search {
    point: Point,
    best: Option<(f64, f64)>,
    work: usize,
}

impl Patch {
    pub(super) fn new(
        patch: &pagura_interpret::Patch,
        values: usize,
        bytes: &mut Meter,
    ) -> Result<Self> {
        let mut colors = super::super::allocate(4 * values, bytes)?;
        for (corner, out) in patch
            .colors
            .iter()
            .zip(colors.chunks_exact_mut(values.max(1)))
        {
            for (out, &v) in out.iter_mut().zip(corner.iter()) {
                *out = v;
            }
        }
        let points = transpose(patch.points);
        Ok(Self {
            points,
            colors: colors.into(),
            regular: injective(&points),
            bounds: Bounds::points(points),
        })
    }

    pub(super) fn bounds(&self) -> Bounds {
        self.bounds
    }

    pub(super) fn bytes(values: usize) -> usize {
        size_of::<Self>() + 4 * values * 8
    }

    /// Bilinear interpolation of the corner values at the parameters of
    /// `p`, if the patch covers it.
    pub(super) fn sample(&self, p: Point, values: usize, out: &mut Input) -> bool {
        if !self.bounds.contains(p) {
            return false;
        }
        let mut best = if self.regular {
            inverse(&self.points, p)
        } else {
            None
        };
        if best.is_none() {
            let mut search = Search {
                point: p,
                best: None,
                work: 0,
            };
            search.run(self.points, [0.0, 0.0, 1.0, 1.0], 0);
            best = search.best;
        }
        let Some((u, v)) = best else {
            return false;
        };
        let corner = |c: usize, i: usize| self.colors.get(c * values + i).copied().unwrap_or(0.0);
        for (i, out) in out.iter_mut().enumerate().take(values) {
            *out = (1.0 - u) * (1.0 - v) * corner(0, i)
                + (1.0 - u) * v * corner(1, i)
                + u * v * corner(2, i)
                + u * (1.0 - v) * corner(3, i);
        }
        true
    }
}

impl Search {
    fn run(&mut self, points: [Point; 16], r: [f64; 4], depth: usize) {
        if self.work >= MAX_WORK {
            return;
        }
        self.work += 1;
        if self
            .best
            .is_some_and(|(u, v)| v > r[3] || (v == r[3] && u >= r[2]))
            || !Bounds::points(points).contains(self.point)
        {
            return;
        }
        if (injective(&points) || depth >= 24)
            && let Some(root) = inverse(&points, self.point)
        {
            self.offer(root, r);
            return;
        }
        // A truly line- or point-valued surface has no area.
        if points.iter().all(|q| q.x == points[0].x) || points.iter().all(|q| q.y == points[0].y) {
            return;
        }
        if depth >= 24 {
            self.offer((0.5, 0.5), r);
            return;
        }
        let (left, right) = split_u(points);
        let (ll, lh) = split_v(left);
        let (rl, rh) = split_v(right);
        let um = r[0].midpoint(r[2]);
        let vm = r[1].midpoint(r[3]);
        for (points, r) in [
            (rh, [um, vm, r[2], r[3]]),
            (lh, [r[0], vm, um, r[3]]),
            (rl, [um, r[1], r[2], vm]),
            (ll, [r[0], r[1], um, vm]),
        ] {
            self.run(points, r, depth + 1);
        }
    }
}

impl Search {
    /// Keeps the root `(u, v)` of box `r` if it lies on a later surface.
    fn offer(&mut self, (u, v): (f64, f64), r: [f64; 4]) {
        let candidate = (r[0] + u * (r[2] - r[0]), r[1] + v * (r[3] - r[1]));
        if self.best.is_none_or(|(u, v)| {
            candidate.1 > v + 1e-9 || ((candidate.1 - v).abs() <= 1e-9 && candidate.0 > u)
        }) {
            self.best = Some(candidate);
        }
    }
}

fn basis(t: f64) -> ([f64; 4], [f64; 4]) {
    let s = 1.0 - t;
    (
        [s * s * s, 3.0 * t * s * s, 3.0 * t * t * s, t * t * t],
        [
            -3.0 * s * s,
            3.0 * s * s - 6.0 * t * s,
            6.0 * t * s - 3.0 * t * t,
            3.0 * t * t,
        ],
    )
}

/// The surface point and its two partial derivatives at `(u, v)`.
fn evaluate(points: &[Point; 16], u: f64, v: f64) -> (Point, Point, Point) {
    let (bu, du) = basis(u);
    let (bv, dv) = basis(v);
    let (mut p, mut a, mut b) = (Point::default(), Point::default(), Point::default());
    for j in 0..4 {
        for i in 0..4 {
            let q = points[j * 4 + i];
            p.x += q.x * bu[i] * bv[j];
            p.y += q.y * bu[i] * bv[j];
            a.x += q.x * du[i] * bv[j];
            a.y += q.y * du[i] * bv[j];
            b.x += q.x * bu[i] * dv[j];
            b.y += q.y * bu[i] * dv[j];
        }
    }
    (p, a, b)
}

fn solve(a: Point, b: Point, p: Point) -> Option<Point> {
    let det = a.x * b.y - a.y * b.x;
    if det == 0.0 || !det.is_finite() {
        return None;
    }
    let p = Point::new((b.y * p.x - b.x * p.y) / det, (a.x * p.y - a.y * p.x) / det);
    (p.x.is_finite() && p.y.is_finite()).then_some(p)
}

fn injective(points: &[Point; 16]) -> bool {
    let (_, a, b) = evaluate(points, 0.5, 0.5);
    let (mut diag, mut cross) = (f64::INFINITY, 0.0_f64);
    for axis in 0..2 {
        for j in 0..if axis == 0 { 4 } else { 3 } {
            for i in 0..if axis == 0 { 3 } else { 4 } {
                let p = points[j * 4 + i];
                let q = points[j * 4 + i + if axis == 0 { 1 } else { 4 }];
                let Some(d) = solve(a, b, Point::new(3.0 * (q.x - p.x), 3.0 * (q.y - p.y))) else {
                    return false;
                };
                diag = diag.min(if axis == 0 { d.x } else { d.y });
                cross = cross.max(if axis == 0 { d.y.abs() } else { d.x.abs() });
            }
        }
    }
    diag > cross + 1e-12
}

fn inverse(points: &[Point; 16], p: Point) -> Option<(f64, f64)> {
    let absolute = points
        .iter()
        .map(|p| p.x.abs().max(p.y.abs()))
        .fold(0.0, f64::max);
    let origin = points[0];
    let points = points.map(|q| Point::new(q.x - origin.x, q.y - origin.y));
    let p = Point::new(p.x - origin.x, p.y - origin.y);
    let scale = points
        .iter()
        .map(|q| q.x.abs().max(q.y.abs()))
        .fold(0.0, f64::max);
    let (mut u, mut v) = (0.5, 0.5);
    for _ in 0..24 {
        let (q, a, b) = evaluate(&points, u, v);
        let error = Point::new(q.x - p.x, q.y - p.y);
        if error.x.abs().max(error.y.abs()) <= scale * 1e-12 + 8.0 * f64::EPSILON * absolute
            && (-1e-10..=1.0 + 1e-10).contains(&u)
            && (-1e-10..=1.0 + 1e-10).contains(&v)
        {
            return Some((u.clamp(0.0, 1.0), v.clamp(0.0, 1.0)));
        }
        let delta = solve(a, b, error)?;
        u -= delta.x;
        v -= delta.y;
        if !(-2.0..=3.0).contains(&u) || !(-2.0..=3.0).contains(&v) {
            return None;
        }
    }
    None
}

fn mix(a: Point, b: Point, t: f64) -> Point {
    Point::new(a.x * (1.0 - t) + b.x * t, a.y * (1.0 - t) + b.y * t)
}

fn split_curve(p: [Point; 4]) -> ([Point; 4], [Point; 4]) {
    let a = mix(p[0], p[1], 0.5);
    let b = mix(p[1], p[2], 0.5);
    let c = mix(p[2], p[3], 0.5);
    let d = mix(a, b, 0.5);
    let e = mix(b, c, 0.5);
    let f = mix(d, e, 0.5);
    ([p[0], a, d, f], [f, e, c, p[3]])
}

fn split_u(p: [Point; 16]) -> ([Point; 16], [Point; 16]) {
    let (mut a, mut b) = ([Point::default(); 16], [Point::default(); 16]);
    for j in 0..4 {
        let (x, y) = split_curve([p[4 * j], p[4 * j + 1], p[4 * j + 2], p[4 * j + 3]]);
        a[4 * j..4 * j + 4].copy_from_slice(&x);
        b[4 * j..4 * j + 4].copy_from_slice(&y);
    }
    (a, b)
}

fn transpose(p: [Point; 16]) -> [Point; 16] {
    let mut t = [Point::default(); 16];
    for (i, out) in t.iter_mut().enumerate() {
        *out = p[(i % 4) * 4 + i / 4];
    }
    t
}

fn split_v(p: [Point; 16]) -> ([Point; 16], [Point; 16]) {
    let (a, b) = split_u(transpose(p));
    (transpose(a), transpose(b))
}
