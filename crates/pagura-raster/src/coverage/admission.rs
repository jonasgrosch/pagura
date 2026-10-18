//! Keep small closed component contours on the strip sweep.

use super::{Geometry, Shape};
use crate::{Point, Segment};

const SWEEP_RANGE: f64 = 65_536.0;

// The strip sweep integrates primitive polygons once per strip. Scattering a
// union of short component polygons creates repeated local sweeps at overlaps.
// Select from the first contour, then bound all controls in a shape-anchored
// integer frame. Neither admission nor the frame depends on the tile.
pub(super) fn sweep_anchor(shapes: &[Shape], maximum: usize) -> Option<Point> {
    let Some(shape) = shapes.first() else {
        return Some(Point::new(0.0, 0.0));
    };
    let first = match &shape.geometry {
        Geometry::Path(path) => match path.segments().first() {
            Some(Segment::MoveTo(first)) => Some(*first),
            _ => None,
        },
        Geometry::Glyph(glyph) => glyph
            .contours()
            .first()
            .and_then(|c| c.points().first())
            .copied(),
    };
    let Some(first) = first else {
        return Some(Point::new(0.0, 0.0));
    };
    let first = first * shape.transform;
    let anchor = Point::new(first.x.floor(), first.y.floor());
    (anchor.x.is_finite()
        && anchor.y.is_finite()
        && shape.segments() <= maximum
        && small_shape(shape)
        && within_frame(shape, anchor)
        && shapes[1..].iter().all(|clip| {
            clip.segments() <= maximum && (clip.rectangle().is_some() || within_frame(clip, anchor))
        }))
    .then_some(anchor)
}

fn small_shape(shape: &Shape) -> bool {
    match &shape.geometry {
        Geometry::Path(path) => small_first_contour(path.segments()),
        Geometry::Glyph(glyph) => glyph.contours().first().is_none_or(|c| {
            // The vertex count includes MoveTo; Close costs one more.
            if c.is_closed() {
                c.points().len() < 6
            } else {
                glyph.vertices() <= 6
            }
        }),
    }
}

fn small_first_contour(segments: &[Segment]) -> bool {
    for segment in segments.iter().take(6) {
        match segment {
            Segment::CubicTo(..) => return false,
            Segment::Close => return true,
            _ => {}
        }
    }
    segments.len() <= 6
}

// Large extents need page-anchored integer crossings. Bound every control point so a
// later contour or nonrectangular clip cannot escape this admission guard.
// Rectangle clips may be omitted in covered tiles; they never affect admission.
fn within_frame(shape: &Shape, anchor: Point) -> bool {
    let t = shape.transform;
    if !t.is_finite() {
        return false;
    }
    let translation = (t.e - anchor.x).abs().max((t.f - anchor.y).abs());
    let scale = (t.a.abs() + t.c.abs()).max(t.b.abs() + t.d.abs());
    let bound = ((SWEEP_RANGE - translation) / scale).min(SWEEP_RANGE);
    if bound < 0.0 {
        return false;
    }
    let point =
        |p: Point| p.x.is_finite() && p.y.is_finite() && p.x.abs() <= bound && p.y.abs() <= bound;
    match &shape.geometry {
        Geometry::Path(path) => path.segments().iter().all(|segment| match *segment {
            Segment::MoveTo(p) | Segment::LineTo(p) => point(p),
            Segment::CubicTo(a, b, c) => point(a) && point(b) && point(c),
            Segment::Close => true,
        }),
        Geometry::Glyph(glyph) => glyph
            .contours()
            .iter()
            .all(|c| c.points().iter().copied().all(point)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FillRule, PathBuilder, Point, Transform};
    use std::sync::Arc;

    #[test]
    fn glyph_polygons_share_path_admission_counts_and_anchor() {
        let limits = pagura_core::Limits::default();
        for closed in [false, true] {
            for count in 1..9 {
                let mut builder = PathBuilder::new();
                builder.move_to(Point::new(0.25, 0.75));
                for i in 1..count {
                    builder
                        .line_to(Point::new(f64::from(i), f64::from(i % 3)))
                        .unwrap();
                }
                if closed {
                    builder.close();
                }
                let path = Arc::new(builder.finish());
                let glyph = Arc::new(
                    crate::GlyphGeometry::new(&path, [1.0, 0.0, 0.0, 1.0], &limits).unwrap(),
                );
                for origin in [(0, 0), (17, 29), (1 << 30, 1 << 30)] {
                    let transform = Transform::translate(f64::from(origin.0), f64::from(origin.1));
                    let path_shape = Shape {
                        geometry: Geometry::Path(Arc::clone(&path)),
                        transform,
                        rule: FillRule::NonZero,
                    };
                    let glyph_shape = Shape {
                        geometry: Geometry::Glyph(Arc::clone(&glyph)),
                        transform,
                        rule: FillRule::NonZero,
                    };
                    let expected = sweep_anchor(&[path_shape], usize::MAX);
                    assert_eq!(
                        sweep_anchor(std::slice::from_ref(&glyph_shape), usize::MAX),
                        expected
                    );
                    if let Some(anchor) = expected {
                        assert_eq!(anchor, Point::new(f64::from(origin.0), f64::from(origin.1)));
                    }
                    assert!(sweep_anchor(&[glyph_shape], 0).is_none());
                }
            }
        }
    }

    #[test]
    fn component_polygons_use_the_strip_sweep_and_long_contours_accumulate() {
        let mut builder = PathBuilder::new();
        builder.rect(0.0, 0.0, 100.0, 0.25);
        for i in 0..128 {
            builder.rect(f64::from(i), 0.0, 0.5, 0.5);
        }
        let shape = |path| Shape {
            geometry: Geometry::Path(Arc::new(path)),
            transform: Transform::IDENTITY,
            rule: FillRule::NonZero,
        };
        let component = shape(builder.finish());
        assert!(sweep_anchor(&[component], usize::MAX).is_some());
        builder.move_to(Point::new(0.0, 0.0));
        for p in [(1.0, 0.0), (2.0, 0.5), (2.0, 1.0), (1.0, 2.0), (0.0, 1.0)] {
            builder.line_to(Point::new(p.0, p.1)).unwrap();
        }
        builder.close();
        assert!(sweep_anchor(&[shape(builder.finish())], usize::MAX).is_none());
        builder.move_to(Point::new(0.0, 0.0));
        builder
            .cubic_to(
                Point::new(1.0, 0.0),
                Point::new(1.0, 1.0),
                Point::new(0.0, 1.0),
            )
            .unwrap();
        builder.close();
        // A cubic is a single construction command, not a straight edge.
        assert!(sweep_anchor(&[shape(builder.finish())], usize::MAX).is_none());
    }

    #[test]
    fn global_range_and_redundant_rectangle_clips_preserve_admission() {
        let mut builder = PathBuilder::new();
        builder.move_to(Point::new(0.0, 0.0));
        builder.line_to(Point::new(3.0, 0.0)).unwrap();
        builder.line_to(Point::new(0.0, 3.0)).unwrap();
        builder.close();
        let mut triangle = Shape {
            geometry: Geometry::Path(Arc::new(builder.finish())),
            transform: Transform::IDENTITY,
            rule: FillRule::NonZero,
        };
        builder.rect(-1e12, -1e12, 1e12 + 1.5, 2e12);
        let clip = Shape {
            geometry: Geometry::Path(Arc::new(builder.finish())),
            transform: Transform::IDENTITY,
            rule: FillRule::NonZero,
        };
        assert!(sweep_anchor(&[triangle.clone()], usize::MAX).is_some());
        assert!(sweep_anchor(&[triangle.clone(), clip], usize::MAX).is_some());
        assert!(sweep_anchor(&[triangle.clone()], 0).is_none());
        triangle.transform = Transform::translate(1e12, 1e12);
        assert!(sweep_anchor(&[triangle], usize::MAX).is_some());
        builder.move_to(Point::new(-1e12, -1e12));
        builder.line_to(Point::new(1e12, 1e12)).unwrap();
        builder.line_to(Point::new(1e12, -1e12)).unwrap();
        builder.close();
        assert!(
            sweep_anchor(
                &[Shape {
                    geometry: Geometry::Path(Arc::new(builder.finish())),
                    transform: Transform::IDENTITY,
                    rule: FillRule::NonZero
                }],
                usize::MAX
            )
            .is_none()
        );
    }

    #[test]
    fn half_ties_keep_the_same_bytes_at_every_tile_and_integer_translation() {
        let q = 1.0 / f64::from(1_u32 << 22);
        let mut builder = PathBuilder::new();
        builder.move_to(Point::new(-2.0, 5.0 + 3.0 * q));
        builder
            .line_to(Point::new(-3.0 * q, 2.0 + 2.0 * q))
            .unwrap();
        builder
            .line_to(Point::new(7.0 + 3.0 * q, 4.0 - 2.0 * q))
            .unwrap();
        builder.close();
        let path = builder.finish();
        for rule in [FillRule::NonZero, FillRule::EvenOdd] {
            let reference =
                crate::Mask::from_path(8, 8, &path, Transform::IDENTITY, rule, 64).unwrap();
            for origin in [(0, 0), (17, 29), (1 << 30, 1 << 30)] {
                let transform = Transform::translate(f64::from(origin.0), f64::from(origin.1));
                let whole =
                    crate::Mask::from_path_at(8, 8, &path, transform, rule, 64, origin).unwrap();
                assert_eq!(whole.coverage(), reference.coverage());
                for y in 0..8 {
                    for x in 0..8 {
                        let tile = crate::Mask::from_path_at(
                            1,
                            1,
                            &path,
                            transform,
                            rule,
                            1,
                            (origin.0 + x, origin.1 + y),
                        )
                        .unwrap();
                        assert_eq!(tile.coverage()[0], whole.coverage()[(y * 8 + x) as usize]);
                    }
                }
            }
        }
    }
}
