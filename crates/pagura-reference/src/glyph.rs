//! The reference of an outline depends on its source curves, never on the
//! renderer's certificate or choice of kernel. Degree-elevated quadratics
//! use the true curve; general cubics keep their canonical polygon.

use pagura_core::{Error, Limits, Result};
use pagura_raster::{Path, Point, Segment, Transform};

/// The reference of a glyph outline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reference {
    /// The true curve.
    Curve,
    /// The placed polygon at the documented tolerance.
    Polygon,
}

/// The reference of `outline` under its translation-free map.
pub(crate) fn reference(outline: &Path, linear: Transform, limits: &Limits) -> Result<Reference> {
    if !linear.is_finite() {
        return Err(Error::invalid_argument("glyph linear map is not finite"));
    }
    if outline.segments().len() > limits.path_segments {
        return Err(Error::limit("path segment budget exceeded"));
    }
    if linear.a * linear.d - linear.b * linear.c == 0.0 {
        return Ok(Reference::Polygon);
    }
    let (mut start, mut current) = (Point::default(), Point::default());
    let mut difference = [0.0_f64; 2];
    let mut curved = false;
    for segment in outline.segments() {
        match *segment {
            Segment::MoveTo(p) => {
                current = p * linear;
                start = current;
            }
            Segment::LineTo(p) => current = p * linear,
            Segment::CubicTo { c1, c2, end } => {
                let [a, b, c, d] = [current, c1 * linear, c2 * linear, end * linear];
                difference[0] += (-a.x + 3.0 * b.x - 3.0 * c.x + d.x).abs();
                difference[1] += (-a.y + 3.0 * b.y - 3.0 * c.y + d.y).abs();
                current = d;
                curved = true;
            }
            Segment::Close => current = start,
        }
    }
    // A whole-outline rounding allowance, not an allowance per segment.
    let quadratic = difference.into_iter().all(|d| d <= 1.0 / 16_777_216.0);
    Ok(if curved && quadratic {
        Reference::Curve
    } else {
        Reference::Polygon
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pagura_raster::PathBuilder;

    #[test]
    fn quadratic_reference_does_not_depend_on_contour_overlap() {
        let mut path = PathBuilder::new();
        for x in [0.0, 1.0] {
            path.move_to(Point::new(x, 0.0));
            path.cubic_to(
                Point::new(x + 2.0, 4.0),
                Point::new(x + 4.0, 4.0),
                Point::new(x + 6.0, 0.0),
            )
            .unwrap();
            path.close();
        }
        assert_eq!(
            reference(&path.finish(), Transform::IDENTITY, &Limits::default()).unwrap(),
            Reference::Curve
        );
    }

    #[test]
    fn general_cubics_keep_the_polygon_reference() {
        let mut path = PathBuilder::new();
        path.move_to(Point::new(0.0, 0.0));
        path.cubic_to(
            Point::new(2.0, 20.0),
            Point::new(14.0, 24.0),
            Point::new(20.0, 0.0),
        )
        .unwrap();
        path.close();
        assert_eq!(
            reference(&path.finish(), Transform::IDENTITY, &Limits::default()).unwrap(),
            Reference::Polygon
        );
    }
}
