use super::*;
use crate::band::RASTER_WORK;
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
            source: Some(source),
            ..
        } => {
            owned = polygon(
                &source.outline,
                source.linear,
                prep.limits,
                &mut crate::testing::bytes(),
                AREA,
            )
            .unwrap();
            (&owned.0, owned.1.as_slice())
        }
        Kind::Polygon {
            polygon, points, ..
        } => (polygon, points.as_slice()),
        Kind::Curve {
            outline, linear, ..
        } => {
            owned = polygon(
                outline,
                *linear,
                prep.limits,
                &mut crate::testing::bytes(),
                AREA,
            )
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
    rounding::certify(edges, origin, Some(shape), prep)
        .unwrap()
        .unwrap()
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
                "the body and canonical point should share the collected spline"
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

#[derive(Debug, PartialEq, Eq, Hash)]
struct Key;
impl StoreKey for Key {
    type Value = GlyphEdges;
}

#[test]
fn collected_points_are_stable_at_minimum_decoded_budget_and_in_store() {
    let path = outline(Point::new(2.25, 1.25), false);
    let limits = Limits::default();
    assert!(
        curve::spline(
            &path,
            Transform::IDENTITY,
            &limits,
            &mut crate::testing::bytes(),
        )
        .unwrap()
        .is_none()
    );
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
    assert!(lo > 0);
    assert_eq!(build(lo - 1).unwrap_err().kind, ErrorKind::Limit);
    let low = Arc::new(build(lo).unwrap());
    let high = Arc::new(build(limits.decoded_bytes).unwrap());
    let Kind::Curve {
        spline: low_spline, ..
    } = &low.kind
    else {
        panic!("the minimum successful budget must retain the collected spline");
    };
    let Kind::Curve {
        spline: high_spline,
        ..
    } = &high.kind
    else {
        panic!("extra budget must not change the collected geometry");
    };
    assert_eq!(low_spline.points, [Point::new(2.25, 1.25)]);
    assert_eq!(low_spline.points, high_spline.points);
    assert_eq!(low_spline.pieces, high_spline.pieces);
    assert_eq!(low_spline.starts, high_spline.starts);
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
fn source_metadata_is_bounded_and_execution_layouts_are_unchanged() {
    use crate::band::edges::{Polygon, StripList};
    use crate::band::shape::{EdgeBody, Resolved};
    #[expect(dead_code, reason = "only the prior layout is compared")]
    enum OldKind {
        Curve(Spline, Relative, Arc<Path>, Transform),
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
    assert!(size_of::<crate::band::shape::GlyphBody>() <= size_of::<EdgeBody>());
    assert_eq!(size_of::<Spline>(), 3 * size_of::<Vec<usize>>());
    // Source scalar facts are charged in GlyphEdges::new; execution keeps
    // the same inline body and shape, with no new per-placement owner.
    assert!(size_of::<Kind>() <= size_of::<OldKind>() + size_of::<Summary>());
    assert!(size_of::<GlyphEdges>() <= size_of::<OldEdges>() + size_of::<Summary>());
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
        assert_eq!(error.kind, ErrorKind::Limit);
        assert_eq!(
            error.message,
            if limits.raster_work < needed_work {
                RASTER_WORK
            } else {
                RENDER_BYTES
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
    assert_eq!(error.message, RENDER_BYTES);
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
    assert_eq!(error.kind, ErrorKind::Limit);
}

fn overlapping_arch(scale: f64, near_integer_extremum: bool) -> Path {
    let mut path = PathBuilder::new();
    for _ in 0..2 {
        path.move_to(Point::default());
        path.cubic_to(
            Point::new(2.0 * scale, 12.0 * scale),
            Point::new(4.0 * scale, 12.0 * scale),
            Point::new(6.0 * scale, 0.0),
        )
        .unwrap();
        if !near_integer_extremum {
            path.line_to(Point::new(6.0 * scale, 14.0 * scale)).unwrap();
            path.line_to(Point::new(0.0, 14.0 * scale)).unwrap();
        }
        path.close();
    }
    path.finish()
}

#[test]
fn stored_polygon_proof_ignores_the_placement_work_budget() {
    let path = overlapping_arch(1.0, false);
    let limits = Limits::default();
    let high = GlyphEdges::new(&path, [1.0, 0.0, 0.0, 1.0], &limits).unwrap();
    let limits = Limits {
        raster_work: 0,
        ..limits
    };
    let low = GlyphEdges::new(&path, [1.0, 0.0, 0.0, 1.0], &limits).unwrap();
    let Kind::Polygon {
        polygon: low_polygon,
        points: low_points,
        source: Some(low_source),
        ..
    } = &low.kind
    else {
        panic!("overlapping quadratic contours need a polygon proof");
    };
    let Kind::Polygon {
        polygon: high_polygon,
        points: high_points,
        source: Some(high_source),
        ..
    } = &high.kind
    else {
        panic!("extra placement work must not change stored geometry");
    };
    assert_eq!(low_polygon.points, high_polygon.points);
    assert_eq!(low_polygon.starts, high_polygon.starts);
    assert_eq!(low_points, high_points);
    assert_eq!(low.retained_bytes(), high.retained_bytes());
    assert_eq!(low_source.initial_area, high_source.initial_area);
    let low_proof = low_source.proof.as_ref().unwrap();
    let high_proof = high_source.proof.as_ref().unwrap();
    let origin = Point::new(0.137, 0.271);
    for bounds in [
        [0.0, 0.0, 1.0, 1.0],
        [2.0, 2.0, 4.0, 3.0],
        [20.0, 20.0, 21.0, 21.0],
    ] {
        let low = low_proof
            .error(bounds, origin, &mut Meter::new(usize::MAX, RASTER_WORK))
            .unwrap();
        let high = high_proof
            .error(bounds, origin, &mut Meter::new(usize::MAX, RASTER_WORK))
            .unwrap();
        assert_eq!(low.to_bits(), high.to_bits());
    }
    let error = low_proof
        .error(
            [0.0, 0.0, 1.0, 1.0],
            origin,
            &mut Meter::new(0, RASTER_WORK),
        )
        .unwrap_err();
    assert_eq!((error.kind, error.message), (ErrorKind::Limit, RASTER_WORK));
}

#[test]
fn initial_polygon_candidates_keep_original_extrema_and_tiny_loops() {
    let limits = Limits::default();
    for (scale, near_integer_extremum) in [(1.0, false), (1.0 / 3.0, true), (1.0 / 512.0, true)] {
        let path = overlapping_arch(scale, near_integer_extremum);
        let edges = Arc::new(GlyphEdges::new(&path, [1.0, 0.0, 0.0, 1.0], &limits).unwrap());
        assert!(matches!(
            &edges.kind,
            Kind::Polygon {
                source: Some(_),
                ..
            }
        ));
        for band in [1, 3, 16] {
            let setup = options(&limits, band);
            let prep = Prep::new(&setup, None);
            for origin in [
                Point::default(),
                Point::new(0.0, 1e-10),
                Point::new(0.75, 0.999_999_999_9),
            ] {
                let actual = place(&edges, origin, &prep).unwrap();
                let expected = canonical(&edges, origin, &prep);
                let (pixels, spent) = crate::testing::raster(&setup.area, actual.as_ref(), None);
                assert_eq!(
                    pixels,
                    crate::testing::raster(&setup.area, Some(&expected), None).0,
                    "{scale}/{near_integer_extremum}/{band}/{origin:?}"
                );
                if let Some(actual) = &actual {
                    assert!(spent <= actual.work());
                }
            }
        }
    }
}
