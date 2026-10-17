//! Bicubic patches, including folded-surface precedence (8.7.4.5.7–.8).
//! Convex-hull subdivision isolates inverse images. A positive-definite
//! symmetric transformed Jacobian certifies injectivity before accepting a
//! Newton root. Unresolved numerical cases fail explicitly, never drop colour.
use super::{Bounds, Decoder, Primitive, charge, push};
use pagura_core::{Error, Limits, Result};
use pagura_raster::Point;
#[derive(Debug)]
pub(super) struct Patch {
    pub(super) points: [Point; 16],
    colors: [[f64; 32]; 4],
    regular: bool,
    bounds: Bounds,
}
const ORDER: [usize; 16] = [0, 4, 8, 12, 13, 14, 15, 11, 7, 3, 2, 1, 5, 9, 10, 6];

pub(super) fn decode(
    d: &mut Decoder<'_>,
    out: &mut Vec<Primitive>,
    limits: &Limits,
    coons: bool,
) -> Result<()> {
    let mut previous = None;
    while d.more() {
        let flag = usize::from(d.flag()?);
        let mut raw = [Point::default(); 16];
        let mut colors = [[0.0; 32]; 4];
        if flag != 0 {
            let (points, old): ([Point; 16], [[f64; 32]; 4]) =
                previous.ok_or(Error::syntax("patch continuation has no preceding patch"))?;
            for (i, p) in raw.iter_mut().enumerate().take(4) {
                *p = points[(flag * 3 + i) % 12];
            }
            colors[0] = old[flag];
            colors[1] = old[(flag + 1) % 4];
        }
        for p in raw
            .iter_mut()
            .take(if coons { 12 } else { 16 })
            .skip(if flag == 0 { 0 } else { 4 })
        {
            *p = d.point()?;
        }
        for c in colors.iter_mut().skip(if flag == 0 { 0 } else { 2 }) {
            *c = d.color()?;
        }
        d.align();
        previous = Some((raw, colors));
        let mut points = [Point::default(); 16];
        for (i, &index) in ORDER.iter().enumerate() {
            points[index] = raw[i];
        }
        if coons {
            for (i, j) in [(1, 1), (1, 2), (2, 1), (2, 2)] {
                let u = f64::from(i) / 3.0;
                let v = f64::from(j) / 3.0;
                let i = usize::try_from(i).map_err(|_| Error::limit("patch index overflow"))?;
                let j = usize::try_from(j).map_err(|_| Error::limit("patch index overflow"))?;
                let sc = mix(points[i], points[12 + i], v);
                let sd = mix(points[4 * j], points[4 * j + 3], u);
                let sb = mix(
                    mix(points[0], points[3], u),
                    mix(points[12], points[15], u),
                    v,
                );
                points[j * 4 + i] = Point::new(sc.x + sd.x - sb.x, sc.y + sd.y - sb.y);
            }
        }
        push(
            out,
            Primitive::Patch(Box::new(Patch {
                points,
                colors,
                regular: injective(&points),
                bounds: Bounds::points(points),
            })),
            limits,
        )?;
    }
    Ok(())
}
impl Patch {
    pub(super) fn sample(
        &self,
        p: Point,
        n: usize,
        work: &mut usize,
        maximum: usize,
    ) -> Result<Option<[f64; 32]>> {
        if !self.bounds.contains(p) {
            return Ok(None);
        }
        charge(work, maximum)?;
        let mut best = if self.regular {
            inverse(&self.points, p)
        } else {
            None
        };
        if best.is_none() {
            search(
                self.points,
                [0.0, 0.0, 1.0, 1.0],
                p,
                &mut best,
                0,
                work,
                maximum,
            )?;
        }
        let Some((u, v)) = best else { return Ok(None) };
        let mut out = [0.0; 32];
        for (i, out) in out.iter_mut().enumerate().take(n) {
            *out = (1.0 - u) * (1.0 - v) * self.colors[0][i]
                + (1.0 - u) * v * self.colors[1][i]
                + u * v * self.colors[2][i]
                + u * (1.0 - v) * self.colors[3][i];
        }
        Ok(Some(out))
    }
}
fn search(
    points: [Point; 16],
    r: [f64; 4],
    p: Point,
    best: &mut Option<(f64, f64)>,
    depth: usize,
    work: &mut usize,
    maximum: usize,
) -> Result<()> {
    charge(work, maximum)?;
    if best.is_some_and(|(u, v)| v > r[3] || (v == r[3] && u >= r[2]))
        || !Bounds::points(points).contains(p)
    {
        return Ok(());
    }
    // A final parameter box is narrower than 2^-24. Its roots are
    // indistinguishable at our component precision, including singular folds.
    if (injective(&points) || depth >= 24)
        && let Some((u, v)) = inverse(&points, p)
    {
        let candidate = (r[0] + u * (r[2] - r[0]), r[1] + v * (r[3] - r[1]));
        if best.is_none_or(|(u, v)| {
            candidate.1 > v + 1e-9 || ((candidate.1 - v).abs() <= 1e-9 && candidate.0 > u)
        }) {
            *best = Some(candidate);
        }
        return Ok(());
    }
    // A truly line/point-valued surface has no area in this sampling policy.
    if points.iter().all(|q| q.x == points[0].x) || points.iter().all(|q| q.y == points[0].y) {
        return Ok(());
    }
    if depth >= 24 {
        return Err(Error::unsupported(
            crate::features::SHADING_MESH,
            "patch inverse cannot be resolved at numeric precision",
        ));
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
        search(points, r, p, best, depth + 1, work, maximum)?;
    }
    Ok(())
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
    p.is_finite().then_some(p)
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
fn split_v(p: [Point; 16]) -> ([Point; 16], [Point; 16]) {
    let transpose = |p: [Point; 16]| std::array::from_fn(|i| p[(i % 4) * 4 + i / 4]);
    let (a, b) = split_u(transpose(p));
    (transpose(a), transpose(b))
}
