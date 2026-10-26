//! Outlines with one explicit closed move point: the curves of the other
//! contours and the point's floor cell, against the canonical polygon.

use super::*;
use crate::band::RASTER_WORK;
use crate::band::shape::{Body, GlyphGeometry, Winding};
use crate::{Area, ClipShape, DeviceRect, ShapeOptions};
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
    let Kind::Curve { source, .. } = &edges.kind else {
        panic!("a curve body with its point");
    };
    let mut bytes = crate::testing::bytes();
    let (polygon, points) =
        polygon(&source.outline, source.linear, prep.limits, &mut bytes).unwrap();
    assert_eq!(points.len(), 1);
    let shape = translated(
        (&polygon, &points),
        origin,
        Winding::Check(0.0),
        prep,
        &mut bytes,
        None,
        None,
    )
    .unwrap()
    .unwrap();
    rounding::certify(edges, origin, shape, prep).unwrap()
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
            assert_eq!(edges.points(), [point]);
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
fn a_declined_point_projection_refunds_its_pieces() {
    let limits = Limits::default();
    let mut two = PathBuilder::new();
    two.append(&outline(Point::new(2.25, 1.25), false), Transform::IDENTITY);
    two.move_to(Point::new(4.25, 1.25));
    two.close();
    let mut bytes = crate::testing::bytes();
    bytes.charge(41).unwrap();
    assert!(
        curve::point_spline(&two.finish(), Transform::IDENTITY, &limits, &mut bytes)
            .unwrap()
            .is_none()
    );
    assert_eq!(bytes.used(), 41);
    assert!(
        size_of::<crate::band::shape::GlyphBody>() <= size_of::<crate::band::shape::EdgeBody>()
    );
    assert_eq!(size_of::<Spline>(), 2 * size_of::<Vec<usize>>());
}

#[derive(Debug, PartialEq, Eq, Hash)]
struct Key;
impl StoreKey for Key {
    type Value = GlyphEdges;
}

#[test]
fn the_point_projection_is_the_same_across_decoded_limits_and_store_states() {
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
        .get_or_build(Key, || -> Result<_> {
            Ok(Built {
                bytes: low.retained_bytes(),
                value: build(lo)?,
            })
        })
        .unwrap();
    let again = store
        .get_or_build(Key, || -> Result<_> { panic!("warm value") })
        .unwrap();
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
fn only_one_explicit_closed_move_point_is_projected() {
    let limits = Limits::default();
    let linear = Transform::new(2.0, 0.0, 0.0, -3.0, 0.0, 0.0);
    let point = Point::new(2.25, 1.25);
    let path = outline(point, false);
    let build = |path: &Path, keep: bool| {
        let mut bytes = crate::testing::bytes();
        if keep {
            curve::spline(path, linear, &limits, &mut bytes)
                .unwrap()
                .map(|_| None)
        } else {
            curve::point_spline(path, linear, &limits, &mut bytes)
                .unwrap()
                .map(|(_, point)| Some(point))
        }
    };
    assert_eq!(build(&path, true), None);
    // The point as the polygon classifies it: its source point mapped.
    assert_eq!(build(&path, false), Some(Some(point * linear)));
    let (_, points) = polygon(&path, linear, &limits, &mut crate::testing::bytes()).unwrap();
    assert_eq!(points, [point * linear]);
    let declined = |segments: &dyn Fn(&mut PathBuilder)| {
        let mut path = PathBuilder::new();
        segments(&mut path);
        build(&path.finish(), false)
    };
    // A point alone, a point drawn by a segment, two points, and a move
    // that another move follows.
    assert_eq!(
        declined(&|p| {
            p.move_to(point);
            p.close();
        }),
        None
    );
    assert_eq!(
        declined(&|p| {
            p.move_to(point);
            p.line_to(point).unwrap();
            p.close();
        }),
        None
    );
    for second in [true, false] {
        assert_eq!(
            declined(&|p| {
                p.append(&outline(point, false), Transform::IDENTITY);
                p.move_to(Point::new(4.25, 1.25));
                if second {
                    p.close();
                } else {
                    p.move_to(Point::new(5.0, 1.0));
                    p.line_to(Point::new(6.0, 2.0)).unwrap();
                    p.line_to(Point::new(5.0, 3.0)).unwrap();
                    p.close();
                }
            }),
            None
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
    assert_eq!(shape.retained_bytes(), size_of::<Shape>());
    assert!(shape.resolved.is_empty());
    let limits = Limits {
        raster_work: needed_work,
        ..limits
    };
    let setup = options(&limits, 3);
    let same = placed(&edges, Point::default(), &Prep::new(&setup, None))
        .unwrap()
        .unwrap();
    let (_, spent) = crate::testing::raster(&setup.area, Some(&same), None);
    assert!(spent <= needed_work);
    let limits = Limits {
        raster_work: needed_work - 1,
        ..limits
    };
    let setup = options(&limits, 3);
    let error = placed(&edges, Point::default(), &Prep::new(&setup, None)).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
    assert_eq!(error.message, RASTER_WORK);
}
