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

use super::Bounds;
use pagura_raster::Point;
use std::num::NonZeroU8;

/// The most subdivision boxes one sample of one patch visits.
pub(super) const MAX_WORK: usize = 4096;

#[derive(Debug, Clone)]
pub(super) struct Patch {
    /// `p_ij` at `4 · j + i`, `i` indexing `u`: the transpose of the
    /// scene's layout, as the evaluation reads it.
    points: [Point; 16],
    regular: bool,
    bounds: Bounds,
    /// None where the bounds alone test samples.
    hull: Option<Hull>,
}

/// The convex hull of a patch's control points, which holds its surface:
/// each surface point is an affine combination of them with nonnegative
/// Bernstein weights.
#[derive(Debug, Clone, Copy)]
struct Hull {
    /// The indices of its corners in the patch's points, anticlockwise.
    corners: [u8; 14],
    len: NonZeroU8,
}

impl Hull {
    /// Andrew's monotone chain over `points`. A hull of fewer than three
    /// corners (collinear or non-finite points) or of more than fourteen
    /// (control points mostly in convex position) is not kept, nor one that
    /// fills more than 63/64 of `bounds`, as a rectangular patch's does: it
    /// would exclude almost no samples at the cost of a test for each.
    fn new(points: &[Point; 16], bounds: Bounds) -> Option<Self> {
        let rectangle = (bounds.hi.x - bounds.lo.x) * (bounds.hi.y - bounds.lo.y);
        let fills = |twice_area: f64| 32.0 * twice_area > 63.0 * rectangle;
        // The control points extreme along the diagonals, anticlockwise,
        // span a quadrilateral within the hull: where it fills the bounds,
        // so does the hull, which is then not built.
        let mut quad = [points[0]; 4];
        for &p in &points[1..] {
            if p.x + p.y < quad[0].x + quad[0].y {
                quad[0] = p;
            }
            if p.x - p.y > quad[1].x - quad[1].y {
                quad[1] = p;
            }
            if p.x + p.y > quad[2].x + quad[2].y {
                quad[2] = p;
            }
            if p.x - p.y < quad[3].x - quad[3].y {
                quad[3] = p;
            }
        }
        if fills(turn(quad[0], quad[1], quad[2]) + turn(quad[0], quad[2], quad[3])) {
            return None;
        }
        let mut sorted: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
        sorted.sort_unstable_by(|&a, &b| {
            let (a, b) = (points[usize::from(a)], points[usize::from(b)]);
            a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y))
        });
        let at = |i: u8| points[usize::from(i)];
        let mut chain = [0_u8; 32];
        let mut len = 0;
        for &i in &sorted {
            while len >= 2 && turn(at(chain[len - 2]), at(chain[len - 1]), at(i)) <= 0.0 {
                len -= 1;
            }
            chain[len] = i;
            len += 1;
        }
        let lower = len + 1;
        for &i in sorted.iter().rev().skip(1) {
            while len >= lower && turn(at(chain[len - 2]), at(chain[len - 1]), at(i)) <= 0.0 {
                len -= 1;
            }
            chain[len] = i;
            len += 1;
        }
        // The upper chain ends where the lower one began.
        len = len.saturating_sub(1);
        let finite = points.iter().all(|p| p.x.is_finite() && p.y.is_finite());
        let count = u8::try_from(len)
            .ok()
            .filter(|n| finite && (3..=14).contains(n))
            .and_then(NonZeroU8::new)?;
        // Twice its area, as a fan of triangles from its first corner.
        let fan = chain[1..len].windows(2);
        if fills(fan.map(|w| turn(at(chain[0]), at(w[0]), at(w[1]))).sum()) {
            return None;
        }
        let mut corners = [0; 14];
        corners[..len].copy_from_slice(&chain[..len]);
        Some(Self {
            corners,
            len: count,
        })
    }

    /// Whether `p` lies within the margin of the hull of `points`: on the
    /// inner side of every edge, or nearer its line than the margin. The
    /// margin is 2⁻²⁰ of the extent of `bounds`, beyond the roots Newton's
    /// method accepts and the centres of the 2⁻²⁴ boxes that subdivision
    /// yields, which lie within sub-patches' control points and so within
    /// the hull.
    fn holds(&self, points: &[Point; 16], bounds: Bounds, p: Point) -> bool {
        let corners = &self.corners[..usize::from(self.len.get())];
        corners
            .iter()
            .zip(corners.iter().cycle().skip(1))
            .all(|(&a, &b)| {
                let (a, b) = (points[usize::from(a)], points[usize::from(b)]);
                let side = turn(a, b, p);
                side >= 0.0 || {
                    let extent = (bounds.hi.x - bounds.lo.x) + (bounds.hi.y - bounds.lo.y);
                    let margin = extent * f64::powi(2.0, -20);
                    let (dx, dy) = (b.x - a.x, b.y - a.y);
                    side * side <= margin * margin * (dx * dx + dy * dy)
                }
            })
    }
}

/// Twice the signed area of `a`, `b`, `c`: positive where `c` lies left of
/// `a` to `b`.
fn turn(a: Point, b: Point, c: Point) -> f64 {
    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}

/// The state of one sample's subdivision.
struct Search {
    point: Point,
    best: Option<(f64, f64)>,
    work: usize,
}

impl Patch {
    pub(super) fn new(patch: &pagura_interpret::Patch) -> Self {
        let points = transpose(patch.points);
        let bounds = Bounds::points(points);
        Self {
            points,
            regular: injective(&points),
            bounds,
            hull: Hull::new(&points, bounds),
        }
    }

    pub(super) fn bounds(&self) -> Bounds {
        self.bounds
    }

    /// Bytes a patch and its corner values hold.
    pub(super) fn bytes(values: usize) -> usize {
        size_of::<Self>() + 4 * values * 8
    }

    /// Whether a sample at `p` may search this patch: only one within the
    /// bounds of its control points and their convex hull, which hold its
    /// whole surface. Its reservation counts a search where this holds, and
    /// sampling searches nowhere else.
    pub(super) fn holds(&self, p: Point) -> bool {
        self.bounds.contains(p) && self.hull_holds(p)
    }

    /// Whether the patch's hull, if it keeps one, holds `p`.
    fn hull_holds(&self, p: Point) -> bool {
        self.hull
            .as_ref()
            .is_none_or(|hull| hull.holds(&self.points, self.bounds, p))
    }

    /// Whether the patch keeps a hull to test samples against.
    #[cfg(test)]
    pub(super) fn has_hull(&self) -> bool {
        self.hull.is_some()
    }

    /// The surface point at parameters `u`, `v`.
    #[cfg(test)]
    pub(super) fn surface(&self, u: f64, v: f64) -> Point {
        evaluate(&self.points, u, v).0
    }

    /// The parameters of `p` on the patch, if it covers it.
    ///
    /// Newton's method on a regular patch runs before the hull test: its
    /// roots lie within the margin of the hull, and most samples of a
    /// regular patch need nothing else.
    pub(super) fn parameters(&self, p: Point) -> Option<(f64, f64)> {
        if !self.bounds.contains(p) {
            return None;
        }
        let best = if self.regular {
            inverse(&self.points, p)
        } else {
            None
        };
        if best.is_some() || !self.hull_holds(p) {
            return best;
        }
        let mut search = Search {
            point: p,
            best: None,
            work: 0,
        };
        search.run(self.points, [0.0, 0.0, 1.0, 1.0], 0);
        search.best
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
