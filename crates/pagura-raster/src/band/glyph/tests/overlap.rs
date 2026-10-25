//! Overlapping quadratic contours resolved once: every placement keeps the
//! bytes of the per-placement polygon and its resolved rows.

use super::*;
use crate::band::shape::{Body, GlyphGeometry};
use crate::{Area, ClipShape, DeviceRect, ShapeOptions};
use pagura_core::Rect;

/// A circle of four quadratics, counterclockwise or clockwise.
fn blob(path: &mut PathBuilder, (cx, cy, r): (f64, f64, f64), reversed: bool) {
    let p = |x: f64, y: f64| Point::new(cx + x * r, cy + y * r);
    let sign = if reversed { -1.0 } else { 1.0 };
    path.move_to(p(1.0, 0.0));
    for [control, end] in [
        [(1.0, 1.0), (0.0, 1.0)],
        [(-1.0, 1.0), (-1.0, 0.0)],
        [(-1.0, -1.0), (0.0, -1.0)],
        [(1.0, -1.0), (1.0, 0.0)],
    ] {
        let control = p(control.0, sign * control.1);
        let end = p(end.0, sign * end.1);
        path.quad_to(control, end).unwrap();
    }
    path.close();
}

/// Two blobs overlapping in the middle rows, wound alike or opposite.
fn outline(first: bool, second: bool) -> Path {
    let mut path = PathBuilder::new();
    blob(&mut path, (4.0, 4.0, 3.0), first);
    blob(&mut path, (6.5, 7.5, 3.0), second);
    path.finish()
}

/// The same edges placed through the polygon, resolving rows per placement.
fn per_placement(outline: &Path, linear: [f64; 4], limits: &Limits) -> Arc<GlyphEdges> {
    let mut edges = GlyphEdges::new(outline, linear, limits).unwrap();
    let Kind::Polygon { resolved, .. } = &mut edges.kind else {
        panic!("an uncertified polygon");
    };
    assert!(resolved.take().is_some());
    Arc::new(edges)
}

#[test]
fn resolved_overlaps_place_the_bytes_of_resolving_each_placement() {
    let limits = Limits::default();
    let options = ShapeOptions {
        area: Area {
            region: DeviceRect {
                x0: 0,
                y0: 0,
                x1: 32,
                y1: 32,
            },
            band_height: 16,
        },
        limits: &limits,
    };
    let prep = Prep::new(&options, None);
    for (first, second) in [(false, false), (true, true), (false, true)] {
        for linear in [[1.0, 0.0, 0.0, 1.0], [1.5, 0.0, 0.25, 1.25]] {
            let outline = outline(first, second);
            let edges = Arc::new(GlyphEdges::new(&outline, linear, &limits).unwrap());
            assert!(!edges.curved());
            assert!(matches!(
                &edges.kind,
                Kind::Polygon {
                    certified: false,
                    resolved: Some(_),
                    ..
                }
            ));
            let old = per_placement(&outline, linear, &limits);
            for fraction in [0.0, 0.125, 0.3, 0.5, 0.71, 0.97] {
                let origin = Point::new(5.0 + fraction, 6.0 + (fraction * 7.0).fract());
                let shape = place(&edges, origin, &prep).unwrap().unwrap();
                let Body::Glyph(body) = &shape.body else {
                    panic!("resolved sides are shared");
                };
                assert_eq!(body.geometry, GlyphGeometry::Relative);
                let expected = place(&old, origin, &prep).unwrap().unwrap();
                let (actual, work) = crate::testing::raster(&options.area, Some(&shape), None);
                let (bytes, _) = crate::testing::raster(&options.area, Some(&expected), None);
                assert_eq!(actual, bytes, "{first} {second} {linear:?} {origin:?}");
                assert!(work <= shape.work());
            }
        }
    }
}

#[test]
fn clipped_placements_keep_the_polygon() {
    let limits = Limits::default();
    let options = ShapeOptions {
        area: Area {
            region: DeviceRect {
                x0: 0,
                y0: 0,
                x1: 32,
                y1: 32,
            },
            band_height: 16,
        },
        limits: &limits,
    };
    let rectangle = Path::rect(Rect::from_corners(0.0, 0.0, 9.5, 32.0).unwrap());
    let clip = ClipShape::new(
        &[(&rectangle, FillRule::NonZero)],
        Transform::IDENTITY,
        &options,
    )
    .unwrap();
    let prep = Prep::new(&options, Some(&clip));
    let outline = outline(false, false);
    let edges = Arc::new(GlyphEdges::new(&outline, [1.0, 0.0, 0.0, 1.0], &limits).unwrap());
    let old = per_placement(&outline, [1.0, 0.0, 0.0, 1.0], &limits);
    let origin = Point::new(0.25, 0.625);
    let shape = place(&edges, origin, &prep).unwrap().unwrap();
    assert!(matches!(shape.body, Body::Edges(_)));
    let expected = place(&old, origin, &prep).unwrap().unwrap();
    assert_eq!(
        crate::testing::raster(&options.area, Some(&shape), Some(&clip)).0,
        crate::testing::raster(&options.area, Some(&expected), Some(&clip)).0
    );
}

/// Two tall overlapping bars with one small curved corner each: the row
/// resolver cuts their long sides at every row they overlap in.
fn bars() -> Path {
    let mut path = PathBuilder::new();
    for x in [0.0, 2.0] {
        path.move_to(Point::new(x, 0.0));
        path.line_to(Point::new(x + 4.0, 0.0)).unwrap();
        path.line_to(Point::new(x + 4.0, 29.9)).unwrap();
        path.quad_to(Point::new(x + 4.0, 30.0), Point::new(x + 3.9, 30.0))
            .unwrap();
        path.line_to(Point::new(x, 30.0)).unwrap();
        path.close();
    }
    path.finish()
}

#[test]
fn resolved_sides_are_retained_within_the_segment_budget() {
    let limits = Limits::default();
    let outline = bars();
    let edges = GlyphEdges::new(&outline, [1.0, 0.0, 0.0, 1.0], &limits).unwrap();
    let Kind::Polygon {
        polygon,
        resolved: Some(relative),
        ..
    } = &edges.kind
    else {
        panic!("resolved sides");
    };
    let sides = relative.lines.len();
    assert!(sides > polygon.vertices());
    assert!(
        edges.retained_bytes()
            >= polygon.heap_bytes()
                + relative.lines.capacity() * size_of::<Line>()
                + size_of::<Relative>()
    );
    // Sides beyond the budget leave each placement resolving its own rows.
    let tight = Limits {
        path_segments: sides - 1,
        ..limits
    };
    let edges = GlyphEdges::new(&outline, [1.0, 0.0, 0.0, 1.0], &tight).unwrap();
    assert!(matches!(
        edges.kind,
        Kind::Polygon {
            certified: false,
            resolved: None,
            ..
        }
    ));
}
