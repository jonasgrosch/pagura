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
    // Start with an empty hull. Seeding the maximum from the viewport origin
    // would turn geometry entirely above/left of it into a spurious boundary
    // cell after the floor-cell allowance below, forcing needless flattening.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PathBuilder;

    #[test]
    fn disjoint_hulls_are_culled_on_every_side() {
        for (x, y) in [(84.0, 100.0), (100.0, 84.0), (120.0, 100.0), (100.0, 120.0)] {
            let mut path = PathBuilder::new();
            path.rect(x, y, 8.0, 8.0);
            assert_eq!(
                region(
                    &path.finish(),
                    Transform::IDENTITY,
                    (16, 16),
                    (100, 100),
                    &Limits::default()
                )
                .unwrap(),
                None,
            );
        }
        assert_eq!(
            region(
                &PathBuilder::new().finish(),
                Transform::IDENTITY,
                (16, 16),
                (100, 100),
                &Limits::default()
            )
            .unwrap(),
            None,
        );
    }

    #[test]
    fn boundary_point_retains_its_floor_cell() {
        let mut path = PathBuilder::new();
        path.move_to(Point::new(100.0, 100.0)).close();
        assert_eq!(
            region(
                &path.finish(),
                Transform::IDENTITY,
                (16, 16),
                (100, 100),
                &Limits::default()
            )
            .unwrap(),
            Some((100, 100, 1, 1)),
        );
    }
}
