//! Bounded adaptive subdivision in device space.

use crate::{Path, Point, Segment, Transform};
use pagura_core::{Error, Limits, Result};

/// Maximum distance of a flattened control point from its chord, in pixels.
/// The convex-hull property bounds the whole curve by this tolerance.
pub(crate) const TOLERANCE: f64 = 1.0 / 256.0;

#[derive(Debug)]
pub(crate) struct Polyline {
    pub(crate) points: Vec<Point>,
    pub(crate) closed: bool,
    pub(crate) degenerate: bool,
}

impl Polyline {
    pub(crate) fn strokeable(&self, cap: crate::LineCap) -> bool {
        if self.points.len() < 2 && !self.closed {
            return false;
        }
        self.points.windows(2).any(|p| p[0] != p[1]) || cap == crate::LineCap::Round
    }
}

pub(crate) fn valid(point: Point) -> bool {
    // Bounds keep intermediate products and intersections finite. Large finite
    // coordinates are a resource limitation, not malformed geometry.
    point.x.is_finite() && point.y.is_finite()
}

pub(crate) fn bounded(point: Point) -> Result<()> {
    if !valid(point) {
        Err(Error::invalid_argument("path geometry is not finite"))
    } else if point.x.abs().max(point.y.abs()) > 1e12 {
        Err(Error::limit("geometry exceeds numeric working range"))
    } else {
        Ok(())
    }
}

pub(crate) fn flatten(path: &Path, transform: Transform, limits: &Limits) -> Result<Vec<Polyline>> {
    let mut result = Vec::new();
    let count = flatten_into(path, transform, limits, &mut result)?;
    result.truncate(count);
    Ok(result)
}

pub(crate) fn flatten_into(
    path: &Path,
    transform: Transform,
    limits: &Limits,
    result: &mut Vec<Polyline>,
) -> Result<usize> {
    if !transform.is_finite() {
        return Err(Error::invalid_argument("path transform is not finite"));
    }
    if path.segments().len() > limits.path_segments {
        return Err(Error::limit("path segment budget exceeded"));
    }
    let mut contour = 0;
    let mut count = 0;
    let mut current = prepare(result, contour);
    for segment in path.segments() {
        // A line or curve following Close reopens at the subpath start.
        // Preserve the closed contour separately, including its point buffer.
        if current.closed && matches!(segment, Segment::LineTo(_) | Segment::CubicTo(..)) {
            let start = current.points.first().copied();
            contour += 1;
            current = prepare(result, contour);
            current.points.extend(start);
        }
        match *segment {
            Segment::MoveTo(point) => {
                if !current.points.is_empty() {
                    contour += 1;
                    current = prepare(result, contour);
                }
                bounded(point)?;
                bounded(point * transform)?;
                append(&mut current.points, point, &mut count, limits)?;
                current.closed = false;
                current.degenerate = true;
            }
            Segment::LineTo(point) => {
                bounded(point)?;
                bounded(point * transform)?;
                current.degenerate &= current.points.first().is_none_or(|p| *p == point);
                append(&mut current.points, point, &mut count, limits)?;
            }
            Segment::CubicTo(first, second, end) => {
                let start = current
                    .points
                    .last()
                    .copied()
                    .ok_or(Error::invalid_argument("curve has no current point"))?;
                current.degenerate &= [first, second, end].iter().all(|p| *p == start);
                for point in [first, second, end] {
                    bounded(point)?;
                    bounded(point * transform)?;
                }
                curve(
                    [start, first, second, end],
                    transform,
                    0,
                    &mut current.points,
                    &mut count,
                    limits,
                )?;
            }
            Segment::Close => current.closed = true,
        }
    }
    Ok(contour + usize::from(!current.points.is_empty()))
}

fn prepare(result: &mut Vec<Polyline>, index: usize) -> &mut Polyline {
    if index == result.len() {
        result.push(Polyline {
            points: Vec::new(),
            closed: false,
            degenerate: true,
        });
    }
    let current = &mut result[index];
    current.points.clear();
    current.closed = false;
    current.degenerate = true;
    current
}

fn append(points: &mut Vec<Point>, point: Point, count: &mut usize, limits: &Limits) -> Result<()> {
    if *count >= limits.path_segments {
        return Err(Error::limit("flattened segment budget exceeded"));
    }
    *count += 1;
    points.push(point);
    Ok(())
}

fn curve(
    points: [Point; 4],
    transform: Transform,
    depth: usize,
    out: &mut Vec<Point>,
    count: &mut usize,
    limits: &Limits,
) -> Result<()> {
    let [a, b, c, d] = points.map(|p| p * transform);
    if distance(b, a, d).max(distance(c, a, d)) <= TOLERANCE {
        return append(out, points[3], count, limits);
    }
    if depth >= limits.depth {
        return Err(Error::limit("curve subdivision depth exceeded"));
    }
    let [a, b, c, d] = points;
    let ab = midpoint(a, b);
    let bc = midpoint(b, c);
    let cd = midpoint(c, d);
    let abc = midpoint(ab, bc);
    let bcd = midpoint(bc, cd);
    let mid = midpoint(abc, bcd);
    curve([a, ab, abc, mid], transform, depth + 1, out, count, limits)?;
    curve([mid, bcd, cd, d], transform, depth + 1, out, count, limits)
}

fn midpoint(a: Point, b: Point) -> Point {
    Point::new(a.x.midpoint(b.x), a.y.midpoint(b.y))
}

fn distance(p: Point, a: Point, b: Point) -> f64 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let square = dx * dx + dy * dy;
    if square == 0.0 {
        return (p.x - a.x).hypot(p.y - a.y);
    }
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / square).clamp(0.0, 1.0);
    (p.x - a.x - t * dx).hypot(p.y - a.y - t * dy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PathBuilder;

    #[test]
    fn geometric_point_classification_precedes_curve_approximation() {
        let mut path = PathBuilder::new();
        path.move_to(Point::new(4.0, 4.0));
        path.cubic_to(
            Point::new(4.001, 4.0),
            Point::new(4.0, 4.001),
            Point::new(4.0, 4.0),
        )
        .unwrap();
        path.close();
        let contours = flatten(&path.finish(), Transform::IDENTITY, &Limits::default()).unwrap();
        assert_eq!(contours[0].points[0], contours[0].points[1]);
        assert!(!contours[0].degenerate);
    }
}
