use super::*;
use crate::band::shape::Body;
use crate::{Area, ClipShape, DeviceRect, ShapeOptions};
use pagura_core::{Built, Rect, Store, StoreKey};

fn outline(height: f64) -> Path {
    let mut path = PathBuilder::new();
    path.rect(2.0, 1.0, 5.0, height);
    path.move_to(Point::new(4.25, 3.75));
    path.close();
    path.move_to(Point::new(4.25, 3.75));
    path.close();
    path.finish()
}

fn owned(edges: &GlyphEdges, origin: Point, prep: &Prep<'_>, bytes: &mut Meter) -> Result<Shape> {
    let Kind::Polygon {
        polygon,
        points,
        certified,
        ..
    } = &edges.kind
    else {
        panic!("polygon source required");
    };
    translated(
        (polygon, points),
        origin,
        if *certified {
            Winding::Certified
        } else {
            Winding::Check(0.0)
        },
        prep,
        bytes,
        None,
        None,
    )
    .map(|shape| shape.unwrap())
}

fn area(height: u32, band_height: u32) -> Area {
    Area {
        region: DeviceRect {
            x0: 0,
            y0: 0,
            x1: 16,
            y1: height,
        },
        band_height,
    }
}

fn equivalent(
    edges: &Arc<GlyphEdges>,
    origin: Point,
    options: &ShapeOptions<'_>,
    clip: Option<&ClipShape>,
) -> (Shape, Shape) {
    let prep = Prep::new(options, clip);
    let shared = placed(edges, origin, &prep).unwrap().unwrap();
    let copied = owned(edges, origin, &prep, &mut crate::testing::bytes()).unwrap();
    assert_eq!(shared.bounds(), copied.bounds());
    assert_eq!(shared.boundaries(), copied.boundaries());
    assert_eq!(shared.work(), copied.work());
    let (actual, work) = crate::testing::raster(&options.area, Some(&shared), clip);
    let (expected, old_work) = crate::testing::raster(&options.area, Some(&copied), clip);
    assert_eq!(actual, expected);
    assert!(work <= shared.work());
    assert!(old_work <= copied.work());
    (shared, copied)
}

#[test]
fn polygon_placements_share_short_and_indexed_geometry_with_identical_work() {
    let limits = Limits::default();
    for height in [7.0, 37.0] {
        let edges =
            Arc::new(GlyphEdges::new(&outline(height), [1.0, 0.0, 0.0, 1.0], &limits).unwrap());
        assert!(!edges.curved());
        for band in [1, 3, 16, 32] {
            let options = ShapeOptions {
                area: area(48, band),
                limits: &limits,
            };
            for origin in [
                Point::new(-0.25, 0.25),
                Point::new(0.75, -0.25),
                Point::new(0.0, 0.0),
            ] {
                let (shared, copied) = equivalent(&edges, origin, &options, None);
                let Body::Glyph(body) = &shared.body else {
                    panic!("uncut polygon should share");
                };
                assert!(Arc::ptr_eq(&body.edges, &edges));
                assert_eq!(body.strips.is_some(), height > 16.0);
                assert!(shared.retained_bytes() < copied.retained_bytes());
                let exact = Limits {
                    raster_work: copied.work(),
                    ..limits
                };
                assert!(
                    placed(
                        &edges,
                        origin,
                        &Prep::new(
                            &ShapeOptions {
                                limits: &exact,
                                ..options
                            },
                            None
                        )
                    )
                    .is_ok()
                );
            }
        }
    }
}

#[test]
fn cutting_clips_and_rebuilt_polygons_keep_owned_edges() {
    let limits = Limits::default();
    let edges = Arc::new(GlyphEdges::new(&outline(37.0), [1.0, 0.0, 0.0, 1.0], &limits).unwrap());
    for band in [1, 7, 32] {
        let options = ShapeOptions {
            area: area(48, band),
            limits: &limits,
        };
        let rectangle = Path::rect(Rect::from_corners(3.5, 0.0, 12.0, 45.0).unwrap());
        let mut path = PathBuilder::new();
        path.move_to(Point::new(1.0, 0.0));
        path.line_to(Point::new(14.0, 0.0)).unwrap();
        path.line_to(Point::new(4.0, 45.0)).unwrap();
        path.close();
        for path in [&rectangle, &path.finish()] {
            let clip = ClipShape::new(&[(path, FillRule::NonZero)], Transform::IDENTITY, &options)
                .unwrap();
            let (shared, _) = equivalent(&edges, Point::new(0.25, 0.25), &options, Some(&clip));
            assert!(matches!(shared.body, Body::Edges(_)));
        }
        assert!(matches!(
            owned(
                &edges,
                Point::default(),
                &Prep::new(&options, None),
                &mut crate::testing::bytes()
            )
            .unwrap()
            .body,
            Body::Edges(_)
        ));
    }
}

#[test]
fn translated_horizontal_collapse_keeps_the_same_boundaries_and_cells() {
    let limits = Limits::default();
    let mut path = PathBuilder::new();
    path.move_to(Point::new(2.0, 0.0));
    path.line_to(Point::new(4.0, 1e-9)).unwrap();
    path.line_to(Point::new(6.0, 2.0)).unwrap();
    path.line_to(Point::new(2.0, 2.0)).unwrap();
    path.close();
    path.move_to(Point::new(10.25, 0.25));
    path.close();
    let edges = Arc::new(GlyphEdges::new(&path.finish(), [1.0, 0.0, 0.0, 1.0], &limits).unwrap());
    let top = 1_u32 << 29;
    for band in [1, 3, 16, 32] {
        let options = ShapeOptions {
            area: Area {
                region: DeviceRect {
                    x0: 0,
                    y0: top,
                    x1: 16,
                    y1: top + 8,
                },
                band_height: band,
            },
            limits: &limits,
        };
        let (shared, _) = equivalent(&edges, Point::new(0.0, f64::from(top)), &options, None);
        assert!(matches!(shared.body, Body::Glyph(_)));
    }
}

#[test]
fn sharing_refunds_only_dropped_edges_and_never_pins_on_refusal() {
    let limits = Limits::default();
    let edges = Arc::new(GlyphEdges::new(&outline(37.0), [1.0, 0.0, 0.0, 1.0], &limits).unwrap());
    let options = ShapeOptions {
        area: area(48, 7),
        limits: &limits,
    };
    let prep = Prep::new(&options, None);
    let mut old_bytes = crate::testing::bytes();
    let copied = owned(&edges, Point::default(), &prep, &mut old_bytes).unwrap();
    let Kind::Polygon {
        polygon,
        points,
        certified,
        ..
    } = &edges.kind
    else {
        unreachable!()
    };
    let mut shared_bytes = crate::testing::bytes();
    let shared = translated(
        (polygon, points),
        Point::default(),
        if *certified {
            Winding::Certified
        } else {
            Winding::Check(0.0)
        },
        &prep,
        &mut shared_bytes,
        None,
        Some(&edges),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        old_bytes.used() - shared_bytes.used(),
        copied.retained_bytes() - shared.retained_bytes()
    );
    let held = Arc::strong_count(&edges);
    let tight = Limits {
        render_bytes: 0,
        ..limits
    };
    let error = placed(
        &edges,
        Point::default(),
        &Prep::new(
            &ShapeOptions {
                limits: &tight,
                ..options
            },
            None,
        ),
    )
    .unwrap_err();
    assert_eq!(error.message, RENDER_BYTES);
    assert_eq!(Arc::strong_count(&edges), held);
}

#[derive(Hash, PartialEq, Eq)]
struct Key(u8);
impl StoreKey for Key {
    type Value = GlyphEdges;
}

#[test]
fn held_polygon_sources_follow_existing_store_pinning_and_nonadmission() {
    let limits = Limits::default();
    let path = outline(37.0);
    let build = || {
        let value = GlyphEdges::new(&path, [1.0, 0.0, 0.0, 1.0], &limits)?;
        Ok(Built {
            bytes: value.retained_bytes(),
            value,
        })
    };
    let cost = build().unwrap().bytes;
    for capacity in [0, cost - 1, cost] {
        let store = Store::new(capacity);
        let edges = store.get_or_build(Key(1), build).unwrap();
        let options = ShapeOptions {
            area: area(48, 3),
            limits: &limits,
        };
        let (shared, _) = equivalent(&edges, Point::default(), &options, None);
        drop(edges);
        store.clear();
        assert_eq!(
            store.retained_bytes(),
            if capacity >= cost { cost } else { 0 }
        );
        let another = store.get_or_build(Key(2), build).unwrap();
        assert!(store.get(&Key(2)).is_none());
        drop((shared, another));
        store.clear();
        assert_eq!(store.retained_bytes(), 0);
    }
}

#[test]
fn long_horizontal_contours_keep_owned_storage_when_raw_visits_exceed_work() {
    let limits = Limits::default();
    let mut path = PathBuilder::new();
    path.move_to(Point::new(2.0, 2.0));
    for i in 1..=1024 {
        path.line_to(Point::new(2.0 + f64::from(i) / 512.0, 2.0))
            .unwrap();
    }
    path.line_to(Point::new(4.0, 4.0)).unwrap();
    path.line_to(Point::new(2.0, 4.0)).unwrap();
    path.close();
    path.move_to(Point::new(6.25, 3.25));
    path.close();
    let edges = Arc::new(GlyphEdges::new(&path.finish(), [1.0, 0.0, 0.0, 1.0], &limits).unwrap());
    for band in [1, 7, 32] {
        let options = ShapeOptions {
            area: area(16, band),
            limits: &limits,
        };
        let (shape, _) = equivalent(&edges, Point::default(), &options, None);
        assert!(matches!(shape.body, Body::Edges(_)));
    }
}

#[test]
fn shared_polygons_with_a_point_keep_their_cancellation() {
    let limits = Limits::default();
    let mut path = PathBuilder::new();
    path.rect(2.0, 2.0, 6.0, 32.0);
    path.move_to(Point::new(2.0, 2.0));
    path.line_to(Point::new(2.0, 34.0)).unwrap();
    path.line_to(Point::new(8.0, 34.0)).unwrap();
    path.line_to(Point::new(8.0, 2.0)).unwrap();
    path.close();
    path.move_to(Point::new(4.25, 3.75));
    path.close();
    let edges = Arc::new(GlyphEdges::new(&path.finish(), [1.0, 0.0, 0.0, 1.0], &limits).unwrap());
    let options = ShapeOptions {
        area: area(48, 3),
        limits: &limits,
    };
    let (shared, _) = equivalent(&edges, Point::new(0.25, 0.25), &options, None);
    assert!(matches!(shared.body, Body::Glyph(_)));
    let (pixels, _) = crate::testing::raster(&options.area, Some(&shared), None);
    assert_eq!(pixels.iter().filter(|&&byte| byte != 0).count(), 1);
}

#[test]
fn negative_zero_origins_keep_shared_and_owned_cell_arithmetic() {
    let limits = Limits::default();
    let mut path = PathBuilder::new();
    path.rect(-0.0, -0.0, 4.0, 4.0);
    path.move_to(Point::new(-0.0, -0.0));
    path.close();
    let edges = Arc::new(GlyphEdges::new(&path.finish(), [1.0, 0.0, 0.0, 1.0], &limits).unwrap());
    for band in [1, 3, 16, 32] {
        let options = ShapeOptions {
            area: area(16, band),
            limits: &limits,
        };
        equivalent(&edges, Point::new(-0.0, -0.0), &options, None);
    }
    assert!(
        size_of::<crate::band::shape::GlyphBody>() <= size_of::<crate::band::shape::EdgeBody>()
    );
}
