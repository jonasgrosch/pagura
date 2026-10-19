//! Boundaries of convex closed miter strokes (ISO 32000-2, 8.4.3.4).

use super::{add, sub};
use crate::flatten::bounded;
use crate::{PathBuilder, Point};
use pagura_core::{Error, Limits, Result};

// Shorter closed runs keep their components.
pub(super) const MIN_VERTICES: usize = 16;

// Outer and inner boundaries of a closed run, or `None` to retain the
// component construction: when an inner side reverses, a miter is clipped, a
// turn is uncertain, or the directions make multiple revolutions.
pub(super) fn boundaries(
    points: &[Point],
    radius: f64,
    tolerance: f64,
    miter_limit: f64,
    limits: &Limits,
) -> Result<Option<[Vec<Point>; 2]>> {
    if points.len() < MIN_VERTICES || radius == 0.0 {
        return Ok(None);
    }
    let mut work = 0;
    crate::coverage::charge(
        &mut work,
        points.len().saturating_mul(14),
        limits.raster_work,
    )?;
    let magnitude = points
        .iter()
        .fold(radius.max(1.0), |m, p| m.max(p.x.abs()).max(p.y.abs()));
    let resolution = 64.0 * f64::EPSILON * magnitude;
    let Ok(vertices) = u32::try_from(points.len()) else {
        return Ok(None);
    };
    // Keep offset arithmetic well below the existing device flattening bound,
    // including when large source coordinates are magnified by the pen CTM.
    if radius <= resolution || resolution * f64::from(vertices) > tolerance / 1024.0 {
        return Ok(None);
    }
    let Some(orientation) = orientation(points) else {
        return Ok(None);
    };
    let mut directions = buffer(points.len())?;
    for (a, b) in points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .take(points.len())
    {
        let (x, y) = (b.x - a.x, b.y - a.y);
        let length = x.hypot(y);
        directions.push(Point::new(x / length, y / length));
    }
    let filled = fills_interior(points, radius);
    let mut outer = buffer(points.len() * 3)?;
    let mut inner = buffer(if filled { 0 } else { points.len() })?;
    for i in 0..points.len() {
        crate::coverage::charge(&mut work, 1, limits.raster_work)?;
        let a = directions[if i == 0 { directions.len() - 1 } else { i - 1 }];
        let b = directions[i];
        let dot = a.x * b.x + a.y * b.y;
        let denominator = 1.0 + dot;
        // A bounded turn keeps the offset intersection well-conditioned.
        if denominator <= 0.5 {
            return Ok(None);
        }
        let side = radius * orientation;
        let n1 = Point::new(-a.y * side, a.x * side);
        let n2 = Point::new(-b.y * side, b.x * side);
        let m = Point::new((n1.x + n2.x) / denominator, (n1.y + n2.y) / denominator);
        if !m.x.is_finite() || !m.y.is_finite() || m.x.hypot(m.y) > radius * miter_limit {
            return Ok(None);
        }
        if !filled {
            inner.push(add(points[i], m));
        }
        outer.extend([sub(points[i], n1), sub(points[i], m), sub(points[i], n2)]);
    }
    for i in 0..inner.len() {
        let a = inner[i];
        let b = inner[(i + 1) % inner.len()];
        let d = directions[i];
        if (b.x - a.x) * d.x + (b.y - a.y) * d.y <= resolution {
            return Ok(None);
        }
    }
    // Strokes union all subpaths, including oppositely directed contours.
    if orientation < 0.0 {
        outer.reverse();
    } else {
        inner.reverse();
    }
    Ok(Some([outer, inner]))
}

pub(super) fn append(
    out: &mut PathBuilder,
    contours: [Vec<Point>; 2],
    limits: &Limits,
) -> Result<()> {
    // Each nonempty contour adds its points and a close.
    let count = contours
        .iter()
        .filter(|c| !c.is_empty())
        .try_fold(out.len(), |n, c| n.checked_add(c.len() + 1));
    if count.is_none_or(|n| n > limits.path_segments) {
        return Err(Error::limit("stroke outline budget exceeded"));
    }
    for contour in contours {
        if contour.is_empty() {
            continue;
        }
        for (i, p) in contour.into_iter().enumerate() {
            bounded(p)?;
            if i == 0 {
                out.move_to(p);
            } else {
                out.line_to(p);
            }
        }
        out.close();
    }
    Ok(())
}

// A horizontal or vertical chord through an interior point has an endpoint
// within half its length. The box bounds that length. Outward rounding keeps
// this a sufficient condition, including when a span is near twice the radius.
fn fills_interior(points: &[Point], radius: f64) -> bool {
    let Some((&first, rest)) = points.split_first() else {
        return false;
    };
    let (lo, hi) = rest.iter().fold((first, first), |(lo, hi), p| {
        (
            Point::new(lo.x.min(p.x), lo.y.min(p.y)),
            Point::new(hi.x.max(p.x), hi.y.max(p.y)),
        )
    });
    (hi.x - lo.x).next_up().min((hi.y - lo.y).next_up()) * 0.5 <= radius
}

fn buffer(capacity: usize) -> Result<Vec<Point>> {
    let mut points = Vec::new();
    points
        .try_reserve_exact(capacity)
        .map_err(|_| Error::limit("stroke boundary allocation failed"))?;
    Ok(points)
}

// Strict turns and exactly one revolution certify a simple convex polygon.
// The error guard excludes uncertain determinants and collinear subdivision.
fn orientation(points: &[Point]) -> Option<f64> {
    let mut sign = 0.0_f64;
    let mut turns = 0;
    let half = |p: Point| p.y > 0.0 || (p.y == 0.0 && p.x >= 0.0);
    for i in 0..points.len() {
        let p = points[if i == 0 { points.len() - 1 } else { i - 1 }];
        let q = points[i];
        let r = points[(i + 1) % points.len()];
        let a = sub(q, p);
        let b = sub(r, q);
        turns += usize::from(half(a) != half(b));
        let (left, right) = (a.x * b.y, a.y * b.x);
        let cross = left - right;
        if cross.abs() <= 8.0 * f64::EPSILON * (left.abs() + right.abs())
            || (sign != 0.0 && cross.signum() != sign)
        {
            return None;
        }
        sign = cross.signum();
    }
    (turns == 2).then_some(sign)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_uncertain_concave_and_repeated_turns() {
        let rectangle = [
            Point::new(1.0, 1.0),
            Point::new(7.0, 1.0),
            Point::new(7.0, 7.0),
            Point::new(1.0, 7.0),
        ];
        assert_eq!(orientation(&rectangle), Some(1.0));
        assert_eq!(
            orientation(&rectangle.into_iter().rev().collect::<Vec<_>>()),
            Some(-1.0)
        );
        assert!(orientation(&rectangle.repeat(2)).is_none());
        let mut concave = rectangle.to_vec();
        concave.insert(2, Point::new(2.0, 2.0));
        assert!(orientation(&concave).is_none());
        let mut subdivided = rectangle.to_vec();
        subdivided.insert(1, Point::new(4.0, 1.0));
        assert!(orientation(&subdivided).is_none());
        let star: Vec<_> = (0..5)
            .map(|i| {
                let a = f64::from((i * 2) % 5) * std::f64::consts::TAU / 5.0;
                Point::new(a.cos(), a.sin())
            })
            .collect();
        assert!(orientation(&star).is_none());
    }

    fn polygon(angles: impl Iterator<Item = f64>, radius: f64) -> Vec<Point> {
        angles
            .map(|a| Point::new(4.0 + radius * a.cos(), 4.0 + radius * a.sin()))
            .collect()
    }

    fn regular(count: u32, radius: f64) -> Vec<Point> {
        let step = std::f64::consts::TAU / f64::from(count);
        polygon((0..count).map(|i| f64::from(i) * step), radius)
    }

    #[test]
    fn fallbacks_return_none_and_new_work_is_bounded() {
        let limits = Limits::default();
        let points = regular(16, 3.0);
        assert!(
            boundaries(&regular(15, 3.0), 1.0, 1.0 / 256.0, 10.0, &limits)
                .unwrap()
                .is_none()
        );
        // A collapsing inner side, a clipped miter and a negligible radius.
        for (radius, miter) in [(2.95, 10.0), (1.0, 1.0), (1e-20, 10.0)] {
            assert!(
                boundaries(&points, radius, 1.0 / 256.0, miter, &limits)
                    .unwrap()
                    .is_none()
            );
        }
        let work = Limits {
            raster_work: 3,
            ..limits
        };
        assert_eq!(
            boundaries(&points, 1.0, 1.0 / 256.0, 10.0, &work)
                .unwrap_err()
                .kind,
            pagura_core::ErrorKind::Limit
        );
        let contours = boundaries(&points, 1.0, 1.0 / 256.0, 10.0, &limits)
            .unwrap()
            .unwrap();
        let segments = Limits {
            path_segments: 3,
            ..limits
        };
        let mut out = PathBuilder::new();
        assert_eq!(
            append(&mut out, contours, &segments).unwrap_err().kind,
            pagura_core::ErrorKind::Limit
        );
        assert_eq!(out.len(), 0);
    }

    #[test]
    fn filled_interior_uses_one_outer_boundary_and_keeps_uncertain_holes() {
        let limits = Limits::default();
        let points = regular(16, 3.0);
        assert!(!fills_interior(&points, 3.0));
        assert!(fills_interior(&points, 4.0));
        let [outer, inner] = boundaries(&points, 4.0, 1.0 / 256.0, 10.0, &limits)
            .unwrap()
            .unwrap();
        assert_eq!(inner, []);
        let mut out = PathBuilder::new();
        append(&mut out, [outer, inner], &limits).unwrap();
        let mask = crate::Mask::from_path(
            8,
            8,
            &out.finish(),
            crate::Transform::IDENTITY,
            crate::FillRule::NonZero,
            64,
        )
        .unwrap();
        assert_eq!(mask.coverage(), [255; 64]);
        // A short side's inner offset reverses while a large hole remains.
        let step = (std::f64::consts::TAU - 0.01) / 15.0;
        let short = polygon(
            (0..16).map(|i| match i {
                0 => 0.0,
                i => 0.01 + f64::from(i - 1) * step,
            }),
            10.0,
        );
        assert!(!fills_interior(&short, 1.0));
        assert!(
            boundaries(&short, 1.0, 1.0 / 256.0, 10.0, &limits)
                .unwrap()
                .is_none()
        );
    }
}
