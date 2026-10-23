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
    let owned;
    let (polygon, points) = match &edges.kind {
        Kind::Polygon {
            polygon, points, ..
        } => (polygon, points.as_slice()),
        Kind::Curve {
            outline, linear, ..
        } => {
            owned =
                super::super::polygon(outline, *linear, prep.limits, &mut crate::testing::bytes())
                    .unwrap();
            (&owned.0, owned.1.as_slice())
        }
    };
    let shape = translated(
        (polygon, points),
        origin,
        Winding::Check(0.0),
        prep,
        &mut crate::testing::bytes(),
        None,
        None,
    )
    .unwrap()
    .unwrap();
    super::super::rounding::certify(edges, origin, shape, prep).unwrap()
}

#[test]
fn point_cell_preserves_winding_and_floor_origin_across_bands() {
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
                    let relative = edges.relative().unwrap();
                    assert!(shape.resolved.is_empty());
                    assert_eq!(
                        shape.boundaries(),
                        relative.lines.len() + relative.curves.len() + 2
                    );
                    assert_eq!(shape.retained_bytes(), size_of::<Shape>());
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
    assert_eq!(size_of::<curve::Spline>(), 3 * size_of::<Vec<usize>>());
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
fn source_metadata_is_bounded_and_execution_layouts_are_unchanged() {
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
    // Source scalar facts are charged in GlyphEdges::new; execution keeps
    // the same inline body and shape, with no new per-placement owner.
    assert!(size_of::<Kind>() <= size_of::<OldKind>() + size_of::<super::super::Summary>());
    assert!(size_of::<GlyphEdges>() <= size_of::<OldEdges>() + size_of::<super::super::Summary>());
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
fn point_cell_checks_the_complete_work_and_fixed_placement_bytes() {
    let limits = Limits::default();
    let edges = Arc::new(
        GlyphEdges::new(
            &outline(Point::new(2.25, 1.25), false),
            [1.0, 0.0, 0.0, 1.0],
            &limits,
        )
        .unwrap(),
    );
    let setup = options(&limits, 3);
    let shape = placed(&edges, Point::default(), &Prep::new(&setup, None))
        .unwrap()
        .unwrap();
    let needed_work = shape.work();
    let needed_bytes = size_of::<Shape>();
    assert_eq!(shape.retained_bytes(), needed_bytes);
    assert!(shape.resolved.is_empty());
    let limits = Limits {
        raster_work: needed_work,
        render_bytes: needed_bytes,
        ..limits
    };
    let setup = options(&limits, 3);
    let same = placed(&edges, Point::default(), &Prep::new(&setup, None))
        .unwrap()
        .unwrap();
    let (_, spent) = crate::testing::raster(&setup.area, Some(&same), None);
    assert!(spent <= needed_work);
    for limits in [
        Limits {
            raster_work: needed_work - 1,
            ..limits
        },
        Limits {
            render_bytes: needed_bytes - 1,
            ..limits
        },
    ] {
        let setup = options(&limits, 3);
        let error = placed(&edges, Point::default(), &Prep::new(&setup, None)).unwrap_err();
        assert_eq!(error.kind, pagura_core::ErrorKind::Limit);
        assert_eq!(
            error.message,
            if limits.raster_work < needed_work {
                RASTER_WORK
            } else {
                crate::band::RENDER_BYTES
            }
        );
    }
}

#[test]
fn repeated_and_disjoint_point_cells_preserve_both_body_orientations() {
    let limits = Limits::default();
    for reversed in [false, true] {
        for duplicate_count in [1, 2, 4] {
            let first = Point::new(2.25, 1.25);
            let base = outline(first, reversed);
            let mut path = PathBuilder::new();
            for segment in base.segments() {
                match *segment {
                    Segment::MoveTo(p) => {
                        path.move_to(p);
                    }
                    Segment::LineTo(p) => {
                        path.line_to(p).unwrap();
                    }
                    Segment::CubicTo { c1, c2, end } => {
                        path.cubic_to(c1, c2, end).unwrap();
                    }
                    Segment::Close => {
                        path.close();
                    }
                }
            }
            for _ in 1..duplicate_count {
                path.move_to(first);
                path.close();
            }
            for point in [
                Point::new(2.75, 1.75),
                Point::new(8.25, 4.25),
                Point::new(-1.25, 3.25),
            ] {
                path.move_to(point);
                path.close();
            }
            let edges =
                Arc::new(GlyphEdges::new(&path.finish(), [1.0, 0.0, 0.0, 1.0], &limits).unwrap());
            let Kind::Curve { spline, .. } = &edges.kind else {
                panic!("the certified body keeps its canonical point contours");
            };
            assert_eq!(spline.points.len(), duplicate_count + 3);
            for band in [1, 3, 16, 32] {
                let setup = options(&limits, band);
                let prep = Prep::new(&setup, None);
                for origin in [
                    Point::default(),
                    Point::new(1.75, 0.75),
                    Point::new(-0.25, 0.25),
                ] {
                    let actual = place(&edges, origin, &prep).unwrap().unwrap();
                    let expected = canonical(&edges, origin, &prep);
                    let (pixels, spent) = crate::testing::raster(&setup.area, Some(&actual), None);
                    assert_eq!(
                        pixels,
                        crate::testing::raster(&setup.area, Some(&expected), None).0,
                        "{reversed}/{duplicate_count}/{band}/{origin:?}"
                    );
                    assert!(spent <= actual.work(), "{spent}>{}", actual.work());
                    let relative = edges.relative().unwrap();
                    assert_eq!(
                        actual.boundaries(),
                        relative.lines.len() + relative.curves.len() + 2 * edges.points().len()
                    );
                }
            }
        }
    }
}

#[test]
fn linear_body_with_points_charges_retained_shape_without_curve_inspection() {
    let limits = Limits::default();
    let mut path = PathBuilder::new();
    path.rect(0.0, 0.0, 4.0, 4.0);
    path.move_to(Point::new(1.25, 1.25));
    path.close();
    let edges = Arc::new(GlyphEdges::new(&path.finish(), [1.0, 0.0, 0.0, 1.0], &limits).unwrap());
    assert!(matches!(edges.kind, Kind::Curve { .. }));
    let limits = Limits {
        render_bytes: 0,
        ..limits
    };
    let setup = options(&limits, 3);
    let error = placed(&edges, Point::default(), &Prep::new(&setup, None)).unwrap_err();
    assert_eq!(error.message, crate::band::RENDER_BYTES);
}

#[test]
fn mixed_point_range_validation_precedes_empty_clip_culling() {
    let limits = Limits::default();
    let path = outline(
        Point::new(crate::flatten::MAX_COORDINATE - 1.0, 1.25),
        false,
    );
    let edges = Arc::new(GlyphEdges::new(&path, [1.0, 0.0, 0.0, 1.0], &limits).unwrap());
    assert!(matches!(edges.kind, Kind::Curve { .. }));
    let setup = options(&limits, 3);
    let rectangle = Path::rect(Rect::from_corners(40.0, 40.0, 44.0, 44.0).unwrap());
    let clip = ClipShape::new(
        &[(&rectangle, FillRule::NonZero)],
        Transform::IDENTITY,
        &setup,
    )
    .unwrap();
    assert!(clip.bounds().is_empty());
    let prep = Prep::new(&setup, Some(&clip));
    assert!(placed(&edges, Point::default(), &prep).unwrap().is_none());
    let error = placed(&edges, Point::new(2.0, 0.0), &prep).unwrap_err();
    assert_eq!(error.kind, pagura_core::ErrorKind::Limit);
}
