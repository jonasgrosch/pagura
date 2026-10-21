//! Conservative device bounds for culling: control hulls of paths under
//! their transforms, which contain every pixel a mark's exact coverage can
//! reach.

use pagura_raster::{DeviceRect, Path, Point, Segment, Transform};

/// A rectangle in device space, `[x0, y0, x1, y1]`.
pub(crate) type Hull = [f64; 4];

/// The control hull of `points`; `None` when there are none.
pub(crate) fn points(points: impl IntoIterator<Item = Point>) -> Option<Hull> {
    points.into_iter().fold(None, |hull, p| {
        Some(match hull {
            None => [p.x, p.y, p.x, p.y],
            Some([x0, y0, x1, y1]) => [x0.min(p.x), y0.min(p.y), x1.max(p.x), y1.max(p.y)],
        })
    })
}

/// The control hull of `path` under `transform`.
pub(crate) fn path(path: &Path, transform: Transform) -> Option<Hull> {
    points(path.segments().iter().flat_map(|segment| {
        let (a, b, c) = match *segment {
            Segment::MoveTo(p) | Segment::LineTo(p) => (Some(p), None, None),
            Segment::CubicTo { c1, c2, end } => (Some(c1), Some(c2), Some(end)),
            Segment::Close => (None, None, None),
        };
        [a, b, c]
            .into_iter()
            .flatten()
            .map(move |p| transform.apply(p))
    }))
}

/// `hull` grown by `margin` on every side.
pub(crate) fn expand(hull: Hull, margin: f64) -> Hull {
    [
        hull[0] - margin,
        hull[1] - margin,
        hull[2] + margin,
        hull[3] + margin,
    ]
}

/// The pixels of `region` that `hull` can touch. A hull with a non-finite
/// or NaN coordinate is taken as the whole region.
pub(crate) fn device(hull: Option<Hull>, region: DeviceRect) -> DeviceRect {
    let Some([x0, y0, x1, y1]) = hull else {
        return DeviceRect::default();
    };
    if [x0, y0, x1, y1].iter().any(|v| v.is_nan()) {
        return region;
    }
    let low = |v: f64, lo: u32, hi: u32| clamp(v.floor(), lo, hi);
    let high = |v: f64, lo: u32, hi: u32| clamp(v.ceil(), lo, hi);
    let rect = DeviceRect {
        x0: low(x0, region.x0, region.x1),
        y0: low(y0, region.y0, region.y1),
        x1: high(x1, region.x0, region.x1),
        y1: high(y1, region.y0, region.y1),
    };
    if rect.is_empty() {
        DeviceRect::default()
    } else {
        rect
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "an integral value clamped to a u32 range"
)]
fn clamp(value: f64, lo: u32, hi: u32) -> u32 {
    value.clamp(f64::from(lo), f64::from(hi)) as u32
}

/// The bound of the scale of `transform`'s linear part: its Frobenius
/// norm, at least its largest singular value.
pub(crate) fn scale(transform: Transform) -> f64 {
    (transform.a * transform.a
        + transform.b * transform.b
        + transform.c * transform.c
        + transform.d * transform.d)
        .sqrt()
}

/// The unit square under `transform`, as a path.
pub(crate) fn unit_square() -> Path {
    let mut builder = pagura_raster::PathBuilder::new();
    builder.rect(0.0, 0.0, 1.0, 1.0);
    builder.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hulls_round_outwards_and_clamp() {
        let region = DeviceRect {
            x0: 0,
            y0: 0,
            x1: 10,
            y1: 10,
        };
        let hull = path(
            &unit_square(),
            Transform::new(2.5, 0.0, 0.0, 2.0, 1.25, 3.0),
        );
        assert_eq!(hull, Some([1.25, 3.0, 3.75, 5.0]));
        assert_eq!(
            device(hull, region),
            DeviceRect {
                x0: 1,
                y0: 3,
                x1: 4,
                y1: 5
            }
        );
        assert!(device(Some([20.0, 0.0, 30.0, 5.0]), region).is_empty());
        assert_eq!(device(Some([f64::NAN, 0.0, 1.0, 1.0]), region), region);
        assert_eq!(device(Some([-1e300, -1e300, 1e300, 1e300]), region), region);
    }
}
