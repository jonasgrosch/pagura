//! The parameters of a point on a tensor-product patch (8.7.4.5.8).
//!
//! Control points are row-major, `points[4 i + j]` = `p_ij`, so that
//! `S(u, v) = Σ p_ij B_i(u) B_j(v)`. Where the patch folds over itself, the
//! point with the largest `v`, then the largest `u`, paints (8.7.4.5.7).
//! Sub-patches whose control hull holds the point are halved down to a
//! parameter box of 2^-12, where Newton's method finds the exact
//! parameters.

use pagura_raster::Point;

/// Depth at which parameter boxes are solved instead of halved.
const DEPTH: u32 = 12;

/// The parameters `(u, v)` of `p`, if the patch covers it.
pub(super) fn locate(points: &[Point; 16], p: Point) -> Option<(f64, f64)> {
    let mut best = None;
    search(points, points, [0.0, 0.0, 1.0, 1.0], p, 0, &mut best);
    best
}

/// Searches the sub-patch `piece` of `patch` over parameter box `[u0, v0,
/// u1, v1]`, upper boxes first.
fn search(
    patch: &[Point; 16],
    piece: &[Point; 16],
    b: [f64; 4],
    p: Point,
    depth: u32,
    best: &mut Option<(f64, f64)>,
) {
    if best.is_some_and(|(u, v)| b[3] < v || (b[3] == v && b[2] <= u)) || !hull_contains(piece, p) {
        return;
    }
    if depth >= DEPTH {
        if let Some(found) = newton(patch, p, b)
            && best.is_none_or(|(u, v)| found.1 > v || (found.1 == v && found.0 > u))
        {
            *best = Some(found);
        }
        return;
    }
    let (low_u, high_u) = split_u(piece);
    let (um, vm) = (f64::midpoint(b[0], b[2]), f64::midpoint(b[1], b[3]));
    let (ll, lh) = split_v(&low_u);
    let (hl, hh) = split_v(&high_u);
    for (piece, b) in [
        (hh, [um, vm, b[2], b[3]]),
        (lh, [b[0], vm, um, b[3]]),
        (hl, [um, b[1], b[2], vm]),
        (ll, [b[0], b[1], um, vm]),
    ] {
        search(patch, &piece, b, p, depth + 1, best);
    }
}

/// Whether the control points' bounding box, slightly widened, holds `p`.
fn hull_contains(points: &[Point; 16], p: Point) -> bool {
    let (mut low, mut high) = (points[0], points[0]);
    for q in points {
        low = Point::new(low.x.min(q.x), low.y.min(q.y));
        high = Point::new(high.x.max(q.x), high.y.max(q.y));
    }
    let slack = 1e-9 * (high.x - low.x).max(high.y - low.y).max(1e-300);
    p.x >= low.x - slack && p.x <= high.x + slack && p.y >= low.y - slack && p.y <= high.y + slack
}

/// The Bernstein basis of degree three and its derivative at `t`.
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

/// `S(u, v)` and its partial derivatives.
fn evaluate(points: &[Point; 16], u: f64, v: f64) -> (Point, Point, Point) {
    let (bu, du) = basis(u);
    let (bv, dv) = basis(v);
    let (mut s, mut su, mut sv) = (Point::default(), Point::default(), Point::default());
    for i in 0..4 {
        for j in 0..4 {
            let q = points[4 * i + j];
            s += q * (bu[i] * bv[j]);
            su += q * (du[i] * bv[j]);
            sv += q * (bu[i] * dv[j]);
        }
    }
    (s, su, sv)
}

/// Newton's method from the box centre; the root, if it converges inside
/// the slightly widened box.
fn newton(points: &[Point; 16], p: Point, b: [f64; 4]) -> Option<(f64, f64)> {
    let scale = points
        .iter()
        .fold(0.0_f64, |m, q| m.max((*q - points[0]).length()))
        .max(1e-300);
    let (mut u, mut v) = (f64::midpoint(b[0], b[2]), f64::midpoint(b[1], b[3]));
    for _ in 0..32 {
        let (s, su, sv) = evaluate(points, u, v);
        let error = s - p;
        if error.length() <= scale * 1e-12 {
            let slack = 1e-9;
            let inside =
                u >= b[0] - slack && u <= b[2] + slack && v >= b[1] - slack && v <= b[3] + slack;
            return inside.then(|| (u.clamp(0.0, 1.0), v.clamp(0.0, 1.0)));
        }
        let det = su.cross(sv);
        if det == 0.0 || !det.is_finite() {
            return None;
        }
        u -= error.cross(sv) / det;
        v -= su.cross(error) / det;
        if !(u.is_finite() && v.is_finite()) || u < -1.0 || u > 2.0 || v < -1.0 || v > 2.0 {
            return None;
        }
    }
    None
}

fn mix(a: Point, b: Point) -> Point {
    Point::new(f64::midpoint(a.x, b.x), f64::midpoint(a.y, b.y))
}

/// The halves of a cubic.
fn halve([a, b, c, d]: [Point; 4]) -> ([Point; 4], [Point; 4]) {
    let (ab, bc, cd) = (mix(a, b), mix(b, c), mix(c, d));
    let (abc, bcd) = (mix(ab, bc), mix(bc, cd));
    let m = mix(abc, bcd);
    ([a, ab, abc, m], [m, bcd, cd, d])
}

/// The halves in `u` (the first index).
fn split_u(points: &[Point; 16]) -> ([Point; 16], [Point; 16]) {
    let (mut low, mut high) = ([Point::default(); 16], [Point::default(); 16]);
    for j in 0..4 {
        let (l, h) = halve([points[j], points[4 + j], points[8 + j], points[12 + j]]);
        for i in 0..4 {
            low[4 * i + j] = l[i];
            high[4 * i + j] = h[i];
        }
    }
    (low, high)
}

/// The halves in `v` (the second index).
fn split_v(points: &[Point; 16]) -> ([Point; 16], [Point; 16]) {
    let (mut low, mut high) = ([Point::default(); 16], [Point::default(); 16]);
    for i in 0..4 {
        let (l, h) = halve([
            points[4 * i],
            points[4 * i + 1],
            points[4 * i + 2],
            points[4 * i + 3],
        ]);
        low[4 * i..4 * i + 4].copy_from_slice(&l);
        high[4 * i..4 * i + 4].copy_from_slice(&h);
    }
    (low, high)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bilinear patch of a parallelogram, as control points.
    fn flat(origin: Point, along_u: Point, along_v: Point) -> [Point; 16] {
        std::array::from_fn(|k| {
            let (i, j) = (k / 4, k % 4);
            #[expect(clippy::cast_precision_loss, reason = "indices below four")]
            let (u, v) = (i as f64 / 3.0, j as f64 / 3.0);
            origin + along_u * u + along_v * v
        })
    }

    #[test]
    fn points_of_a_flat_patch_have_their_affine_parameters() {
        let points = flat(
            Point::new(1.0, 2.0),
            Point::new(8.0, 1.0),
            Point::new(-2.0, 6.0),
        );
        let p = Point::new(1.0, 2.0) + Point::new(8.0, 1.0) * 0.3 + Point::new(-2.0, 6.0) * 0.7;
        let (u, v) = locate(&points, p).unwrap();
        assert!((u - 0.3).abs() < 1e-9 && (v - 0.7).abs() < 1e-9, "{u} {v}");
        assert!(locate(&points, Point::new(-5.0, -5.0)).is_none());
    }

    #[test]
    fn a_folded_patch_paints_its_largest_v() {
        // v runs out and back along the same segment: v = 1/4 and v = 3/4
        // both reach the point, and the larger wins.
        let mut points = [Point::default(); 16];
        for i in 0..4 {
            #[expect(clippy::cast_precision_loss, reason = "indices below four")]
            let x = i as f64;
            for (j, y) in [0.0, 4.0, 4.0, 0.0].into_iter().enumerate() {
                points[4 * i + j] = Point::new(x, y);
            }
        }
        // S(u, v).y = 12 v (1 - v); 12 · 1/4 · 3/4 = 2.25.
        let (_, v) = locate(&points, Point::new(1.5, 2.25)).unwrap();
        assert!((v - 0.75).abs() < 1e-9, "{v}");
    }
}
