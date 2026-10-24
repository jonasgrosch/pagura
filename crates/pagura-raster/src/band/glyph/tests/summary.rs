use super::*;
use crate::band::shape::{Body, GlyphGeometry};
use crate::{Area, ClipShape, DeviceRect, ShapeOptions};
use pagura_core::Rect;

fn stored(points: Vec<Point>, starts: Vec<usize>) -> Arc<GlyphEdges> {
    let polygon = Polygon {
        points,
        starts,
        points_cells: false,
    };
    assert!(certified(&polygon, 0.0, &mut crate::testing::bytes()).unwrap());
    let summary = Summary::new(&polygon).unwrap();
    Arc::new(GlyphEdges {
        direct: None,
        retained: size_of::<GlyphEdges>() + polygon.heap_bytes(),
        hull: polygon.bounds().map(|[a, b]| [a.x, a.y, b.x, b.y]),
        kind: Kind::Polygon {
            polygon,
            points: Vec::new(),
            certified: true,
            source: None,
            relative: None,
            summary: Some(summary),
        },
    })
}

fn copied(edges: &Arc<GlyphEdges>, origin: Point, prep: &Prep<'_>) -> Option<Shape> {
    let Kind::Polygon {
        polygon, points, ..
    } = &edges.kind
    else {
        panic!("polygon source");
    };
    translated(
        (polygon, points),
        origin,
        Winding::Certified,
        prep,
        &mut crate::testing::bytes(),
        None,
        Some(edges),
    )
    .unwrap()
}

fn pixels(area: &Area, shape: Option<&Shape>, clip: Option<&ClipShape>) -> Vec<u8> {
    shape.map_or_else(
        || vec![0; area.region.width() as usize * area.region.height() as usize],
        |shape| {
            let (pixels, work) = crate::testing::raster(area, Some(shape), clip);
            assert!(work <= shape.work(), "work {work} > {}", shape.work());
            pixels
        },
    )
}

fn config(region: DeviceRect, band_height: u32, limits: &Limits) -> ShapeOptions<'_> {
    ShapeOptions {
        area: Area {
            region,
            band_height,
        },
        limits,
    }
}

#[test]
fn source_summary_layout() {
    #[expect(dead_code, reason = "the frozen source layout is compared")]
    enum Before {
        Curve {
            spline: Spline,
            relative: Relative,
            outline: Arc<Path>,
            linear: Transform,
        },
        Polygon {
            polygon: Polygon,
            points: Vec<Point>,
            certified: bool,
            source: Option<(Arc<Path>, Transform)>,
            relative: Option<Box<Relative>>,
        },
    }
    #[expect(dead_code, reason = "the frozen source layout is compared")]
    struct BeforeEdges {
        retained: usize,
        hull: Option<[f64; 4]>,
        kind: Before,
    }
    eprintln!(
        "summary={} optional={} Kind={} before={} GlyphEdges={} before_edges={} Shape={}",
        size_of::<Summary>(),
        size_of::<Option<Summary>>(),
        size_of::<Kind>(),
        size_of::<Before>(),
        size_of::<GlyphEdges>(),
        size_of::<BeforeEdges>(),
        size_of::<Shape>(),
    );
    assert_eq!(size_of::<Option<Summary>>(), size_of::<Summary>());
    assert!(size_of::<Kind>() <= size_of::<Before>() + size_of::<Summary>());
    assert!(size_of::<GlyphEdges>() <= size_of::<BeforeEdges>() + size_of::<Summary>());
}

#[test]
fn source_summary_bounds_rectangle_pixels_and_work_across_windows() {
    // Non-quadratic collinear controls retain an ordinary polygon whose
    // covered set is this exact rectangle.
    let mut path = PathBuilder::new();
    path.move_to(Point::new(2.0, 2.0));
    path.cubic_to(
        Point::new(3.0, 2.0),
        Point::new(9.5, 2.0),
        Point::new(12.0, 2.0),
    )
    .unwrap();
    path.line_to(Point::new(12.0, 25.0)).unwrap();
    path.line_to(Point::new(2.0, 25.0)).unwrap();
    path.close();
    let limits = Limits::default();
    let edges = Arc::new(GlyphEdges::new(&path.finish(), [1.0, 0.0, 0.0, 1.0], &limits).unwrap());
    assert!(matches!(
        edges.kind,
        Kind::Polygon {
            summary: Some(_),
            ..
        }
    ));
    for origin in [
        Point::new(0.0, 0.0),
        Point::new(-0.25, 0.5),
        Point::new(1.75, 2.25),
    ] {
        for region in [
            crate::testing::window(32, 40),
            DeviceRect {
                x0: 4,
                y0: 9,
                x1: 18,
                y1: 29,
            },
        ] {
            let mut before = None;
            for band in [1, 3, 16, 32, 256] {
                let options = config(region, band, &limits);
                let prep = Prep::new(&options, None);
                let shape = placed(&edges, origin, &prep).unwrap().unwrap();
                let Body::Glyph(body) = &shape.body else {
                    panic!("shared polygon");
                };
                assert_eq!(body.geometry, GlyphGeometry::Polygon);
                assert!(body.strips.is_none());
                assert!(shape.resolved.is_empty());
                assert_eq!(shape.retained_bytes(), size_of::<Shape>());
                let facts = (shape.bounds(), shape.work(), shape.retained_bytes());
                if let Some(before) = before {
                    assert_eq!(facts, before);
                }
                before = Some(facts);
                let actual = pixels(&options.area, Some(&shape), None);
                let expected: Vec<_> = (region.y0..region.y1)
                    .flat_map(|y| {
                        (region.x0..region.x1).map(move |x| {
                            let width = (f64::from(x + 1).min(12.0 + origin.x)
                                - f64::from(x).max(2.0 + origin.x))
                            .clamp(0.0, 1.0);
                            let height = (f64::from(y + 1).min(25.0 + origin.y)
                                - f64::from(y).max(2.0 + origin.y))
                            .clamp(0.0, 1.0);
                            crate::quantize_unit(width * height)
                        })
                    })
                    .collect();
                assert_eq!(actual, expected);
                assert_eq!(
                    actual,
                    pixels(&options.area, copied(&edges, origin, &prep).as_ref(), None)
                );
                let exact = Limits {
                    render_bytes: shape.retained_bytes(),
                    raster_work: shape.work(),
                    ..limits
                };
                let tight = config(region, band, &exact);
                assert!(placed(&edges, origin, &Prep::new(&tight, None)).is_ok());
                for low in [
                    Limits {
                        render_bytes: exact.render_bytes - 1,
                        ..exact
                    },
                    Limits {
                        raster_work: exact.raster_work - 1,
                        ..exact
                    },
                ] {
                    let low = config(region, band, &low);
                    assert_eq!(
                        placed(&edges, origin, &Prep::new(&low, None))
                            .unwrap_err()
                            .kind,
                        ErrorKind::Limit
                    );
                }
            }
        }
    }
}

#[test]
fn source_summary_raw_extents_keep_zero_padding_collapse_and_signs() {
    let limits = Limits::default();
    let cases = [
        stored(
            vec![
                Point::new(-0.0, -0.0),
                Point::new(8.0, 0.0),
                Point::new(8.0, 23.0),
                Point::new(0.0, 23.0),
                Point::new(1024.0, 4.0),
                Point::new(-2.0, 4.0),
            ],
            vec![0, 4],
        ),
        stored(
            vec![
                Point::new(20.0, 2.0),
                Point::new(28.0, 2.0),
                Point::new(28.0, 8.0),
                Point::new(20.0, 8.0),
                Point::new(2.0, 4.0),
                Point::new(30.0, 4.0),
            ],
            vec![0, 4],
        ),
        stored(
            vec![
                Point::new(2.0, -0.0),
                Point::new(4.0, 1e-9),
                Point::new(6.0, 2.0),
                Point::new(2.0, 2.0),
            ],
            vec![0],
        ),
    ];
    for edges in cases {
        for origin in [
            Point::new(-0.0, -0.0),
            Point::new(-0.25, 0.75),
            Point::new(0.0, f64::from(1_u32 << 29)),
        ] {
            let top = if origin.y > 1.0 { 1_u32 << 29 } else { 0 };
            for band in [1, 3, 16, 32, 256] {
                let options = config(
                    DeviceRect {
                        x0: 0,
                        y0: top,
                        x1: 16,
                        y1: top + 32,
                    },
                    band,
                    &limits,
                );
                let prep = Prep::new(&options, None);
                let actual = placed(&edges, origin, &prep).unwrap();
                let expected = copied(&edges, origin, &prep);
                assert_eq!(
                    pixels(&options.area, actual.as_ref(), None),
                    pixels(&options.area, expected.as_ref(), None)
                );
                if let Some(actual) = &actual {
                    assert!(matches!(&actual.body, Body::Glyph(body) if body.strips.is_none()));
                }
            }
        }
    }
}

#[test]
fn source_summary_validates_all_raw_vertices_before_an_empty_clip() {
    let edges = stored(
        vec![
            Point::new(0.0, 0.0),
            Point::new(8.0, 0.0),
            Point::new(8.0, 8.0),
            Point::new(0.0, 8.0),
            Point::new(1024.0, 4.0),
            Point::new(1030.0, 4.0),
        ],
        vec![0, 4],
    );
    let limits = Limits::default();
    let options = config(crate::testing::window(16, 16), 3, &limits);
    let empty = Path::rect(Rect::from_corners(100.0, 100.0, 101.0, 101.0).unwrap());
    let clip = ClipShape::new(
        &[(&empty, FillRule::NonZero)],
        Transform::IDENTITY,
        &options,
    )
    .unwrap();
    assert!(clip.bounds().is_empty());
    let origin = Point::new(crate::flatten::MAX_COORDINATE - 128.0, 0.0);
    let error = placed(&edges, origin, &Prep::new(&options, Some(&clip))).unwrap_err();
    assert_eq!(
        (error.kind, error.message),
        (ErrorKind::Limit, "geometry exceeds numeric working range")
    );
}

#[test]
fn source_summary_full_glyph_dimensions_keep_indexed_setup_for_outliers() {
    let limits = Limits::default();
    for (width, height) in [(8.0, 80.0), (4100.0, 8.0)] {
        let edges = stored(
            vec![
                Point::default(),
                Point::new(width, 0.0),
                Point::new(width, height),
                Point::new(0.0, height),
            ],
            vec![0],
        );
        let Kind::Polygon {
            summary: Some(summary),
            ..
        } = &edges.kind
        else {
            panic!("summary");
        };
        assert!(!Summary::short(
            summary.placed_hull(Point::new(0.25, 0.25)).unwrap()
        ));
        for band in [1, 3, 16, 32, 256] {
            // A one-row crop must not turn the whole source into a short glyph.
            let options = config(
                DeviceRect {
                    x0: 0,
                    y0: 4,
                    x1: 16,
                    y1: 5,
                },
                band,
                &limits,
            );
            let prep = Prep::new(&options, None);
            let actual = placed(&edges, Point::new(0.25, 0.25), &prep)
                .unwrap()
                .unwrap();
            let expected = copied(&edges, Point::new(0.25, 0.25), &prep).unwrap();
            assert_eq!(actual.work(), expected.work());
            assert_eq!(
                pixels(&options.area, Some(&actual), None),
                pixels(&options.area, Some(&expected), None)
            );
        }
    }
    let huge = Polygon {
        points: vec![
            Point::default(),
            Point::new(1e12, 0.0),
            Point::new(1e12, 1e12),
        ],
        starts: vec![0],
        points_cells: false,
    };
    assert!(Summary::new(&huge).is_none());
    assert!(!Summary::short([
        Point::default(),
        Point::new(f64::INFINITY, 1.0)
    ]));
}
