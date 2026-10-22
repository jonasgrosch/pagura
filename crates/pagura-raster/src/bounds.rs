//! Conservative device bounds from the convex hull of path control points.

use crate::flatten::{bounded, validate};
use crate::{Path, Point, Segment, Transform};
use pagura_core::{Limits, Result};

/// The device-space corners `[min, max]` of the hull of `path`'s points
/// and control points under `transform`; inverted (min above max) for a path
/// without points.
pub(crate) fn control_hull(
    path: &Path,
    transform: Transform,
    limits: &Limits,
) -> Result<[Point; 2]> {
    validate(path, transform, limits)?;
    let mut min = Point::new(f64::INFINITY, f64::INFINITY);
    let mut max = Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    let mut record = |point: Point| -> Result<()> {
        bounded(point)?;
        let point = point * transform;
        bounded(point)?;
        min.x = min.x.min(point.x);
        min.y = min.y.min(point.y);
        max.x = max.x.max(point.x);
        max.y = max.y.max(point.y);
        Ok(())
    };
    for segment in path.segments() {
        match *segment {
            Segment::MoveTo(p) | Segment::LineTo(p) => record(p)?,
            Segment::CubicTo { c1, c2, end } => {
                record(c1)?;
                record(c2)?;
                record(end)?;
            }
            Segment::Close => {}
        }
    }
    Ok([min, max])
}
