//! Conservative device bounds from the convex hull of path control points.

use crate::flatten::bounded;
use crate::{Path, Point, Segment, Transform};
use pagura_core::{Error, Limits, Result};

pub(crate) fn region(
    path: &Path,
    transform: Transform,
    size: (u32, u32),
    origin: (u32, u32),
    limits: &Limits,
) -> Result<Option<(u32, u32, u32, u32)>> {
    if !transform.is_finite() {
        return Err(Error::invalid_argument("path transform is not finite"));
    }
    if path.segments().len() > limits.path_segments {
        return Err(Error::limit("path segment budget exceeded"));
    }
    let right = origin
        .0
        .checked_add(size.0)
        .ok_or(Error::limit("tile extent overflows u32"))?;
    let bottom = origin
        .1
        .checked_add(size.1)
        .ok_or(Error::limit("tile extent overflows u32"))?;
    let mut min = Point::new(f64::from(right), f64::from(bottom));
    let mut max = Point::new(f64::from(origin.0), f64::from(origin.1));
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
            Segment::CubicTo(a, b, c) => {
                record(a)?;
                record(b)?;
                record(c)?;
            }
            Segment::Close => {}
        }
    }
    let x0 = coordinate(min.x.floor(), origin.0, right);
    let y0 = coordinate(min.y.floor(), origin.1, bottom);
    // Include the floor cell even at an integral maximum: a true point subpath
    // encloses a device pixel under 8.5.3.3.1. Ordinary bounds remain conservative.
    let x1 = coordinate(max.x.floor() + 1.0, origin.0, right);
    let y1 = coordinate(max.y.floor() + 1.0, origin.1, bottom);
    Ok(if x0 < x1 && y0 < y1 {
        Some((x0, y0, x1 - x0, y1 - y0))
    } else {
        None
    })
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to u32 tile boundaries before casting"
)]
fn coordinate(value: f64, min: u32, max: u32) -> u32 {
    value.clamp(f64::from(min), f64::from(max)) as u32
}
