use super::*;
use crate::band::glyph::{place, placed, translated};
use crate::band::shape::{Body, GlyphGeometry, Winding};
use crate::{Area, ClipShape, DeviceRect, FillRule, PathBuilder, ShapeOptions};
use pagura_core::{Built, Rect, Store, StoreKey};

mod windows;

fn outline(point: Point, reversed: bool) -> Path {
    let mut path = PathBuilder::new();
    if reversed {
        path.move_to(Point::new(0.0, 7.0));
        path.line_to(Point::new(6.0, 7.0)).unwrap();
        path.line_to(Point::new(6.0, 0.0)).unwrap();
        path.cubic_to(
            Point::new(4.0, 4.0),
            Point::new(2.0, 4.0),
            Point::new(0.0, 0.0),
        )
        .unwrap();
    } else {
        path.move_to(Point::new(0.0, 0.0));
        path.cubic_to(
            Point::new(2.0, 4.0),
            Point::new(4.0, 4.0),
            Point::new(6.0, 0.0),
        )
        .unwrap();
        path.line_to(Point::new(6.0, 7.0)).unwrap();
        path.line_to(Point::new(0.0, 7.0)).unwrap();
    }
    path.close();
    path.move_to(point);
    path.close();
    path.finish()
}

fn options(limits: &Limits, band_height: u32) -> ShapeOptions<'_> {
    ShapeOptions {
        area: Area {
            region: DeviceRect {
                x0: 0,
                y0: 0,
                x1: 12,
                y1: 12,
            },
            band_height,
        },
        limits,
    }
}

fn canonical(edges: &Arc<GlyphEdges>, origin: Point, prep: &Prep<'_>) -> Shape {
    let Kind::Polygon {
        polygon, points, ..
    } = &edges.kind
    else {
        panic!("canonical polygon retained");
    };
    let shape = translated(
        (polygon, points),
        origin,
        Winding::Check(0.0),
        prep,
        &mut crate::testing::bytes(),
        None,
        Some(edges),
    )
    .unwrap()
    .unwrap();
    super::super::rounding::certify(edges, origin, shape, prep).unwrap()
}

#[test]
fn point_row_preserves_winding_and_floor_origin_across_bands() {
    let limits = Limits::default();
    for reversed in [false, true] {
        for point in [
            Point::new(2.25, 1.25),
            Point::new(2.25, 3.25),
            Point::new(-1.25, 1.25),
            Point::new(-0.0, -0.0),
            Point::new(-1.0, -1.0),
        ] {
            let path = outline(point, reversed);
            let edges = Arc::new(GlyphEdges::new(&path, [1.0, 0.0, 0.0, 1.0], &limits).unwrap());
            assert!(
                edges.relative().is_some(),
                "projection should fit canonical overlap"
            );
            for band in [1, 3, 16, 32] {
                let options = options(&limits, band);
                let prep = Prep::new(&options, None);
                for origin in [
                    Point::new(2.0, 2.0),
                    Point::new(2.75, 1.25),
                    Point::new(-0.25, 0.75),
                ] {
                    let shape = place(&edges, origin, &prep)
                        .unwrap_or_else(|error| {
                            panic!("{point:?}/{origin:?}/{reversed}: {error:?}")
                        })
                        .unwrap();
                    let expected = canonical(&edges, origin, &prep);
                    let (actual, work) = crate::testing::raster(&options.area, Some(&shape), None);
                    let (old, _) = crate::testing::raster(&options.area, Some(&expected), None);
                    assert_eq!(actual, old, "{point:?}/{origin:?}/{reversed}");
                    assert!(work <= shape.work(), "{work}>{}", shape.work());
                    let Body::Glyph(body) = &shape.body else {
                        panic!("relative body");
                    };
                    assert_eq!(body.geometry, GlyphGeometry::Relative);
                    let row = (point.y + origin.y).floor();
                    if (0.0..12.0).contains(&row) && point.x + origin.x >= 0.0 {
                        let row = (0_u32..12).find(|&y| f64::from(y) == row).unwrap();
                        assert!(shape.resolved.contains(row));
                        let Kind::Polygon {
                            polygon, points, ..
                        } = &edges.kind
                        else {
                            panic!("canonical polygon retained");
                        };
                        let canonical_inputs = PolygonLines::new(polygon, points, origin)
                            .indexed()
                            .filter(|(_, line)| !line.is_horizontal())
                            .count();
                        let relative = edges.relative().unwrap();
                        let relative_inputs = relative.lines.len() + relative.curves.len();
                        // Even sides outside this row contributed to the
                        // canonical preparation's placement-error bound.
                        let resolved_sides = shape.resolved.lines(&(0..12)).len();
                        assert_eq!(
                            shape.boundaries(),
                            canonical_inputs.max(relative_inputs) + resolved_sides
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn rectangle_and_path_cuts_keep_the_canonical_fallback() {
    let limits = Limits::default();
    let edges = Arc::new(
        GlyphEdges::new(
            &outline(Point::new(2.25, 1.25), false),
            [1.0, 0.0, 0.0, 1.0],
            &limits,
        )
        .unwrap(),
    );
    let options = options(&limits, 3);
    let rectangle = Path::rect(Rect::from_corners(2.25, 0.0, 8.0, 8.0).unwrap());
    let mut triangle = PathBuilder::new();
    triangle.move_to(Point::new(0.0, 0.0));
    triangle.line_to(Point::new(8.0, 0.0)).unwrap();
    triangle.line_to(Point::new(0.0, 8.0)).unwrap();
    triangle.close();
    for path in [&rectangle, &triangle.finish()] {
        let clip =
            ClipShape::new(&[(path, FillRule::NonZero)], Transform::IDENTITY, &options).unwrap();
        let prep = Prep::new(&options, Some(&clip));
        let shape = placed(&edges, Point::new(1.0, 1.0), &prep)
            .unwrap()
            .unwrap();
        assert!(matches!(&shape.body, Body::Edges(_)));
        let expected = canonical(&edges, Point::new(1.0, 1.0), &prep);
        assert_eq!(
            crate::testing::raster(&options.area, Some(&shape), Some(&clip)).0,
            crate::testing::raster(&options.area, Some(&expected), Some(&clip)).0,
        );
    }
}

#[test]
fn fixed_constructor_overlap_refunds_declined_projection_and_keeps_layout() {
    let limits = Limits::default();
    let path = outline(Point::new(2.25, 1.25), false);
    let mut bytes = Meter::new(limits.decoded_bytes, "parent");
    bytes.charge(41).unwrap();
    let before = bytes.used();
    assert!(
        relative(&path, Transform::IDENTITY, &limits, 1, &mut bytes)
            .unwrap()
            .is_none()
    );
    assert_eq!(bytes.used(), before);
    assert!(
        size_of::<crate::band::shape::GlyphBody>() <= size_of::<crate::band::shape::EdgeBody>()
    );
    assert_eq!(size_of::<curve::Spline>(), 2 * size_of::<Vec<usize>>());
}

#[test]
fn an_empty_resolved_row_still_masks_the_nonpoint_body() {
    let limits = Limits::default();
    let mut path = PathBuilder::new();
    path.move_to(Point::default());
    path.cubic_to(
        Point::new(2.0, 4.0),
        Point::new(4.0, 4.0),
        Point::new(6.0, 0.0),
    )
    .unwrap();
    path.line_to(Point::new(6.0, 7.0)).unwrap();
    path.line_to(Point::new(0.0, 7.0)).unwrap();
    path.close();
    let edges = Arc::new(GlyphEdges::new(&path.finish(), [1.0, 0.0, 0.0, 1.0], &limits).unwrap());
    let options = options(&limits, 3);
    let prep = Prep::new(&options, None);
    let mut shape = placed(&edges, Point::default(), &prep).unwrap().unwrap();
    let mut bytes = crate::testing::bytes();
    bytes.charge(shape.retained_bytes()).unwrap();
    let mut polygon = crate::band::edges::Polygon::default();
    let mut storage = crate::testing::bytes();
    polygon.begin(&limits, 4, &mut storage).unwrap();
    polygon.points.extend([
        Point::new(0.0, 0.0),
        Point::new(0.0, 1.0),
        Point::new(1.0, 1.0),
        Point::new(1.0, 0.0),
    ]);
    polygon.end();
    // Negative unit-square body and positive canonical point cell cancel.
    let points = [Point::new(0.25, 0.25)];
    let mut work = Meter::new(limits.raster_work, RASTER_WORK);
    shape
        .point_row(
            PolygonLines::new(&polygon, &points, Point::default()),
            0,
            &prep,
            &mut bytes,
            &mut work,
        )
        .unwrap();
    assert!(shape.resolved.contains(0));
    assert_eq!(shape.resolved.lines(&(0..1)), []);
    assert_eq!(bytes.used(), shape.retained_bytes());
    let (actual, spent) = crate::testing::raster(&options.area, Some(&shape), None);
    assert!(actual[..12].iter().all(|&c| c == 0));
    assert!(spent <= shape.work());
}

#[derive(Debug, PartialEq, Eq, Hash)]
struct Key;
impl StoreKey for Key {
    type Value = GlyphEdges;
}

#[test]
fn fixed_overlap_selects_the_same_projection_across_decoded_limits_and_store_states() {
    let path = outline(Point::new(2.25, 1.25), false);
    let limits = Limits::default();
    let build = |decoded_bytes| {
        GlyphEdges::new(
            &path,
            [1.0, 0.0, 0.0, 1.0],
            &Limits {
                decoded_bytes,
                ..limits
            },
        )
    };
    let mut lo = 0;
    let mut hi = limits.decoded_bytes;
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if build(mid).is_ok() {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    let low = Arc::new(build(lo).unwrap());
    let high = Arc::new(build(limits.decoded_bytes).unwrap());
    assert!(low.curved() && high.curved());
    let store = Store::new(limits.decoded_bytes);
    let warm = store
        .get_or_build(Key, || {
            Ok(Built {
                bytes: low.retained_bytes(),
                value: build(lo)?,
            })
        })
        .unwrap();
    let again = store.get_or_build(Key, || panic!("warm value")).unwrap();
    assert!(Arc::ptr_eq(&warm, &again));
    let options = options(&limits, 3);
    let prep = Prep::new(&options, None);
    let origin = Point::new(1.137, 2.271);
    let shapes =
        [&low, &high, &warm, &again].map(|edges| place(edges, origin, &prep).unwrap().unwrap());
    for shape in &shapes[1..] {
        assert_eq!(
            (shape.bounds(), shape.work(), shape.retained_bytes()),
            (
                shapes[0].bounds(),
                shapes[0].work(),
                shapes[0].retained_bytes()
            )
        );
        assert_eq!(
            crate::testing::raster(&options.area, Some(shape), None).0,
            crate::testing::raster(&options.area, Some(&shapes[0]), None).0
        );
    }
}

#[test]
fn only_explicit_closed_move_points_are_omitted() {
    let limits = Limits::default();
    let path = outline(Point::new(2.25, 1.25), false);
    assert!(
        curve::spline(
            &path,
            Transform::IDENTITY,
            &limits,
            &mut crate::testing::bytes()
        )
        .unwrap()
        .is_none()
    );
    assert!(
        curve::nonpoint_spline(
            &path,
            Transform::IDENTITY,
            &limits,
            &mut crate::testing::bytes()
        )
        .unwrap()
        .is_some()
    );
    let mut path = PathBuilder::new();
    path.move_to(Point::new(2.0, 2.0));
    path.close();
    assert!(
        curve::nonpoint_spline(
            &path.finish(),
            Transform::IDENTITY,
            &limits,
            &mut crate::testing::bytes()
        )
        .unwrap()
        .is_none()
    );
    let mut path = PathBuilder::new();
    path.move_to(Point::new(2.0, 2.0));
    path.line_to(Point::new(2.0, 2.0)).unwrap();
    path.close();
    assert!(
        curve::nonpoint_spline(
            &path.finish(),
            Transform::IDENTITY,
            &limits,
            &mut crate::testing::bytes()
        )
        .unwrap()
        .is_none()
    );
}

#[test]
fn old_geometry_and_shape_layouts_are_unchanged() {
    use crate::band::edges::{Polygon, StripList};
    use crate::band::shape::{EdgeBody, Resolved};
    #[expect(dead_code, reason = "only the prior layout is compared")]
    enum OldKind {
        Curve(curve::Spline, Relative, Arc<Path>, Transform),
        Polygon(Polygon, Vec<Point>, bool, Option<(Arc<Path>, Transform)>),
    }
    #[expect(dead_code, reason = "only the prior layout is compared")]
    struct OldEdges {
        retained: usize,
        hull: Option<[f64; 4]>,
        kind: OldKind,
    }
    #[expect(dead_code, reason = "only the prior layout is compared")]
    struct OldGlyph {
        edges: Arc<GlyphEdges>,
        origin: Point,
        scan: (usize, usize),
        strips: Option<StripList>,
        boundaries: usize,
    }
    #[expect(dead_code, reason = "only the prior layout is compared")]
    enum OldBody {
        Edges(EdgeBody),
        Glyph(OldGlyph),
    }
    #[expect(dead_code, reason = "only the prior layout is compared")]
    struct OldShape {
        area: Area,
        bounds: DeviceRect,
        work: usize,
        retained: usize,
        rule: FillRule,
        body: OldBody,
        clip: Option<ClipShape>,
        masked: bool,
        resolved: Resolved,
    }
    assert_eq!(size_of::<Kind>(), size_of::<OldKind>());
    assert_eq!(size_of::<GlyphEdges>(), size_of::<OldEdges>());
    assert_eq!(size_of::<Body>(), size_of::<OldBody>());
    assert_eq!(size_of::<Shape>(), size_of::<OldShape>());
    eprintln!(
        "layouts Kind={} GlyphEdges={} Body={} Shape={}",
        size_of::<Kind>(),
        size_of::<GlyphEdges>(),
        size_of::<Body>(),
        size_of::<Shape>()
    );
}

#[test]
fn point_row_refuses_before_scratch_and_refunds_failed_temporary_owners() {
    let limits = Limits::default();
    let edges = Arc::new(
        GlyphEdges::new(
            &outline(Point::new(2.25, 1.25), false),
            [1.0, 0.0, 0.0, 1.0],
            &limits,
        )
        .unwrap(),
    );
    let Kind::Polygon {
        polygon, points, ..
    } = &edges.kind
    else {
        panic!("polygon");
    };
    let relative = edges.relative().unwrap();
    let options = options(&limits, 3);
    let prep = Prep::new(&options, None);
    let frame = Frame::new(&prep).unwrap();
    let origin = Point::default();
    let hull = relative.placed_hull(origin, Some(points[0])).unwrap();
    for no_work in [false, true] {
        let mut shape = Shape::translated(&edges, relative, origin, hull, &frame, &prep)
            .unwrap()
            .unwrap();
        let before = (shape.work(), shape.retained_bytes());
        let mut bytes = Meter::new(
            if no_work {
                limits.render_bytes
            } else {
                before.1
            },
            crate::band::RENDER_BYTES,
        );
        bytes.charge(before.1).unwrap();
        let mut work = Meter::new(if no_work { 0 } else { limits.raster_work }, RASTER_WORK);
        let error = shape
            .point_row(
                PolygonLines::new(polygon, points, origin),
                1,
                &prep,
                &mut bytes,
                &mut work,
            )
            .unwrap_err();
        assert_eq!(error.kind, pagura_core::ErrorKind::Limit);
        assert_eq!(
            error.message,
            if no_work {
                RASTER_WORK
            } else {
                crate::band::RENDER_BYTES
            }
        );
        assert!(shape.resolved.is_empty());
        assert_eq!((shape.work(), shape.retained_bytes()), before);
        assert_eq!(bytes.used(), before.1);
    }
}
