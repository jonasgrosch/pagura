//! Bounded adaptive subdivision in device space.

use crate::{Path, Point, Segment, Transform};
use pagura_core::{Error, Result};

/// Maximum distance of a flattened control point from its chord, in pixels.
/// The convex-hull property bounds the whole curve by this tolerance.
pub(crate) const TOLERANCE: f64 = 1.0 / 256.0;
pub(crate) const MAX_SEGMENTS: usize = 65_536;

#[derive(Debug)]
pub(crate) struct Polyline {
    pub(crate) points: Vec<Point>,
    pub(crate) closed: bool,
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

pub(crate) fn flatten(path: &Path, transform: Transform) -> Result<Vec<Polyline>> {
    if !transform.is_finite() {
        return Err(Error::invalid_argument("path transform is not finite"));
    }
    if path.segments().len() > MAX_SEGMENTS {
        return Err(Error::limit("path segment budget exceeded"));
    }
    let mut result = Vec::new();
    let mut points = Vec::new();
    let mut closed = false;
    let mut count = 0;
    for segment in path.segments() {
        match *segment {
            Segment::MoveTo(point) => {
                if !points.is_empty() {
                    result.push(Polyline {
                        points: std::mem::take(&mut points),
                        closed,
                    });
                }
                bounded(point)?;
                bounded(point * transform)?;
                append(&mut points, point, &mut count)?;
                closed = false;
            }
            Segment::LineTo(point) => {
                bounded(point)?;
                bounded(point * transform)?;
                reopen(&mut points, &mut closed, &mut result);
                append(&mut points, point, &mut count)?;
            }
            Segment::CubicTo(first, second, end) => {
                reopen(&mut points, &mut closed, &mut result);
                let start = points
                    .last()
                    .copied()
                    .ok_or(Error::invalid_argument("curve has no current point"))?;
                for point in [first, second, end] {
                    bounded(point)?;
                    bounded(point * transform)?;
                }
                curve(
                    [start, first, second, end],
                    transform,
                    0,
                    &mut points,
                    &mut count,
                )?;
            }
            Segment::Close => closed = true,
        }
    }
    if !points.is_empty() {
        result.push(Polyline { points, closed });
    }
    Ok(result)
}

// A segment following Close starts at the original subpath start. Preserve the
// already closed contour separately rather than drawing from its last vertex.
fn reopen(points: &mut Vec<Point>, closed: &mut bool, result: &mut Vec<Polyline>) {
    if *closed {
        let start = points.first().copied();
        result.push(Polyline {
            points: std::mem::take(points),
            closed: true,
        });
        points.extend(start);
        *closed = false;
    }
}

fn append(points: &mut Vec<Point>, point: Point, count: &mut usize) -> Result<()> {
    if *count >= MAX_SEGMENTS {
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
) -> Result<()> {
    let [a, b, c, d] = points.map(|p| p * transform);
    if distance(b, a, d).max(distance(c, a, d)) <= TOLERANCE {
        return append(out, points[3], count);
    }
    if depth >= 32 {
        return Err(Error::limit("curve subdivision depth exceeded"));
    }
    let [a, b, c, d] = points;
    let ab = midpoint(a, b);
    let bc = midpoint(b, c);
    let cd = midpoint(c, d);
    let abc = midpoint(ab, bc);
    let bcd = midpoint(bc, cd);
    let mid = midpoint(abc, bcd);
    curve([a, ab, abc, mid], transform, depth + 1, out, count)?;
    curve([mid, bcd, cd, d], transform, depth + 1, out, count)
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
