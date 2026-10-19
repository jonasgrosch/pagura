//! Select the strip sweep from geometry, independently of the viewport.

use super::{Geometry, Shape};
use crate::{Point, Segment};

const SWEEP_RANGE: f64 = 65_536.0;
// Construction commands in a small component path, not per-cell density.
const SMALL_COMPONENT_SEGMENTS: usize = 512;

/// The most construction segments (MoveTo, lines and Close) a first contour
/// may have and still count as a short component polygon.
const SHORT_CONTOUR: usize = 6;

// The strip sweep integrates primitive polygons once per strip. Scattering a
// union of short component polygons creates repeated local sweeps at overlaps.
// Select from the first contour, then bound all controls in a shape-anchored
// integer frame. Neither admission nor the frame depends on the tile.
pub(super) fn sweep_anchor(shapes: &[Shape], maximum: usize) -> Option<Point> {
    let Some(shape) = shapes.first() else {
        return Some(Point::default());
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
        return Some(Point::default());
    };
    let first = first * shape.transform;
    let anchor = Point::new(first.x.floor(), first.y.floor());
    (anchor.x.is_finite()
        && anchor.y.is_finite()
        && shape.segments() <= maximum
        && prefers_sweep(shape)
        && within_frame(shape, anchor)
        && shapes[1..].iter().all(|clip| {
            clip.segments() <= maximum && (clip.rectangle().is_some() || within_frame(clip, anchor))
        }))
    .then_some(anchor)
}

fn prefers_sweep(shape: &Shape) -> bool {
    match &shape.geometry {
        Geometry::Path(path) => {
            // Long slanted quadrilateral bands create many vertex events in
            // the strip sweep. Cell accumulation localizes their intersections.
            // Keep short, axis-aligned and mixed component paths on the sweep.
            small_first_contour(path.segments())
                && (path.segments().len() <= SMALL_COMPONENT_SEGMENTS
                    || !diagonal_quadrilateral_components(path.segments(), shape.transform))
        }
        Geometry::Glyph(glyph) => glyph.contours().first().is_none_or(|c| {
            // The vertex count includes MoveTo; Close costs one more.
            if c.is_closed() {
                c.points().len() < SHORT_CONTOUR
            } else {
                glyph.vertices() <= SHORT_CONTOUR
            }
        }),
    }
}

fn diagonal_quadrilateral_components(segments: &[Segment], transform: crate::Transform) -> bool {
    use Segment::{Close, LineTo, MoveTo};
    // Device translation cannot change edge orientation or backend selection.
    let transform = crate::Transform {
        e: 0.0,
        f: 0.0,
        ..transform
    };
    let (components, rest) = segments.as_chunks::<5>();
    rest.is_empty()
        && components.iter().all(|c| {
            let [MoveTo(a), LineTo(b), LineTo(c), LineTo(d), Close] = c else {
                return false;
            };
            let [a, b, c, d] = [*a, *b, *c, *d].map(|p| p * transform);
            !((a.x == b.x && b.y == c.y && c.x == d.x && d.y == a.y)
                || (a.y == b.y && b.x == c.x && c.y == d.y && d.x == a.x))
        })
}

fn small_first_contour(segments: &[Segment]) -> bool {
    for segment in segments.iter().take(SHORT_CONTOUR) {
        match segment {
            Segment::CubicTo { .. } => return false,
            Segment::Close => return true,
            _ => {}
        }
    }
    segments.len() <= SHORT_CONTOUR
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
            Segment::CubicTo { c1, c2, end } => point(c1) && point(c2) && point(end),
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
    fn quadrilateral_admission_is_global_and_keeps_other_components_on_sweep() {
        let limits = pagura_core::Limits::default();
        let band = |count: usize, mixed: bool| {
            let mut builder = PathBuilder::new();
            for i in 0..count {
                builder.move_to(Point::new(0.0, 0.0));
                builder.line_to(Point::new(1.0, 0.25)).unwrap();
                builder.line_to(Point::new(1.0, 1.25)).unwrap();
                if !mixed || i != 1 {
                    builder.line_to(Point::new(0.0, 1.0)).unwrap();
                }
                builder.close();
            }
            Arc::new(builder.finish())
        };
        for origin in [(0, 0), (17, 29), (1 << 30, 1 << 30)] {
            let transform = Transform::translate(f64::from(origin.0), f64::from(origin.1));
            for (path, sweep) in [
                (band(102, false), true),
                (band(128, false), false),
                (band(128, true), true),
            ] {
                let shape = Shape {
                    geometry: Geometry::Path(path),
                    transform,
                    rule: FillRule::NonZero,
                };
                assert_eq!(
                    sweep_anchor(&[shape], limits.path_segments).is_some(),
                    sweep
                );
            }
            let path = band(128, false);
            let image = crate::Mask::from_path(
                crate::IntRect::new(origin.0, origin.1, 2, 2).unwrap(),
                Arc::clone(&path),
                transform,
                FillRule::NonZero,
                limits,
            )
            .unwrap();
            // Integral of 1-x/4 and x/4 over one unit-wide pixel column.
            assert_eq!(image.coverage(), [223, 0, 32, 0]);
            let glyph = Arc::new(
                crate::GlyphGeometry::new(&path, Transform::IDENTITY, &limits, usize::MAX).unwrap(),
            );
            let shape = Shape {
                geometry: Geometry::Glyph(glyph),
                transform,
                rule: FillRule::NonZero,
            };
            assert!(sweep_anchor(&[shape], limits.path_segments).is_some());
        }
    }

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
                    crate::GlyphGeometry::new(&path, Transform::IDENTITY, &limits, usize::MAX)
                        .unwrap(),
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
        let mask = |x, y, size, transform, rule| {
            let limits = pagura_core::Limits {
                pixels: 64,
                ..pagura_core::Limits::default()
            };
            let area = crate::IntRect::new(x, y, size, size).unwrap();
            crate::Mask::from_path(area, path.clone(), transform, rule, limits).unwrap()
        };
        for rule in [FillRule::NonZero, FillRule::EvenOdd] {
            let reference = mask(0, 0, 8, Transform::IDENTITY, rule);
            for origin in [(0, 0), (17, 29), (1 << 30, 1 << 30)] {
                let transform = Transform::translate(f64::from(origin.0), f64::from(origin.1));
                let whole = mask(origin.0, origin.1, 8, transform, rule);
                assert_eq!(whole.coverage(), reference.coverage());
                for (i, &value) in whole.coverage().iter().enumerate() {
                    let (x, y) = (i % 8, i / 8);
                    let x = origin.0 + u32::try_from(x).unwrap();
                    let y = origin.1 + u32::try_from(y).unwrap();
                    assert_eq!(mask(x, y, 1, transform, rule).coverage(), [value]);
                }
            }
        }
    }
}
