use super::*;
use crate::{Area, ClipShape, DeviceRect, PathBuilder, ShapeOptions};
use pagura_core::Limits;

#[test]
fn an_empty_clip_keeps_a_tight_mesh_extremum_range_error() {
    let limits = Limits::default();
    let mut path = PathBuilder::new();
    for _ in 0..2 {
        path.move_to(Point::default());
        path.cubic_to(
            Point::new(2.0 / 3000.0, 1.0 / 3.0),
            Point::new(2.0 / 3000.0, 2.0 / 3.0),
            Point::new(0.0, 1.0),
        )
        .unwrap();
        path.close();
    }
    // The doubled narrow quadratic is not a common-sign unit winding.
    // Its initial chords have x=0; the tight mesh keeps its positive
    // x extremum, whose placement exceeds the working range.
    let edges = std::sync::Arc::new(
        GlyphEdges::new(&path.finish(), [1.0, 0.0, 0.0, 1.0], &limits).unwrap(),
    );
    let origin = Point::new(MAX_COORDINATE - 0.00025, 0.0);
    let area = Area {
        region: DeviceRect {
            x0: 0,
            y0: 0,
            x1: 1,
            y1: 1,
        },
        band_height: 1,
    };
    let mut clip_path = PathBuilder::new();
    clip_path.rect(2.0, 2.0, 1.0, 1.0);
    let clip = ClipShape::new(
        &[(&clip_path.finish(), FillRule::NonZero)],
        Transform::IDENTITY,
        &ShapeOptions {
            area,
            limits: &limits,
        },
    )
    .unwrap();
    let options = Prep {
        area,
        limits: &limits,
        clip: Some(&clip),
    };
    assert!(Frame::new(&options).is_none());
    let candidate = super::super::placed(&edges, origin, &options).unwrap();
    assert!(candidate.is_none());
    let mut work = Meter::new(limits.raster_work, RASTER_WORK);
    let source = Source::of(&edges, origin, &mut work).unwrap().unwrap();
    assert!(source.initial_area > AREA);
    let mut bytes = Meter::new(limits.render_bytes, RENDER_BYTES);
    let baseline = tight_placement(
        source,
        &options,
        Meters {
            work: &mut work,
            bytes: &mut bytes,
        },
    )
    .unwrap_err();
    assert_eq!(baseline.kind, pagura_core::ErrorKind::Limit);
    assert_eq!(
        certify(&edges, origin, candidate, &options).unwrap_err(),
        baseline
    );
}

#[test]
fn mixed_rows_keep_original_curve_and_point_enclosures() {
    let limits = Limits::default();
    let origin = Point::new(0.0, 11.0 / 18.0 + 1e-10);
    for reversed in [false, true] {
        let mut path = PathBuilder::new();
        if reversed {
            path.move_to(Point::new(0.0, 7.0));
            path.line_to(Point::new(6.0, 7.0)).unwrap();
            path.line_to(Point::new(6.0, 0.0)).unwrap();
            path.cubic_to(Point::new(4.0, 4.0), Point::new(2.0, 4.0), Point::default())
                .unwrap();
        } else {
            path.move_to(Point::default());
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
        path.move_to(Point::new(2.25, 4.25));
        path.close();
        let path = path.finish();
        let edges =
            std::sync::Arc::new(GlyphEdges::new(&path, [1.0, 0.0, 0.0, 1.0], &limits).unwrap());
        for band_height in [1, 3, 16, 32] {
            let options = Prep {
                area: Area {
                    region: DeviceRect {
                        x0: 0,
                        y0: 0,
                        x1: 8,
                        y1: 10,
                    },
                    band_height,
                },
                limits: &limits,
                clip: None,
            };
            let shape = super::super::placed(&edges, origin, &options)
                .unwrap()
                .unwrap();
            assert!(shape.resolved.is_empty());
            let relative = edges.relative().unwrap();
            assert_eq!(
                shape.boundaries(),
                relative.lines.len() + relative.curves.len() + 2
            );
            let ordinary = placement_error(&path, Transform::IDENTITY, origin, &shape);
            let expected = 384.0
                * f64::EPSILON
                * (7.0 + origin.y)
                * f64::from(u32::try_from(path.segments().len().max(shape.boundaries())).unwrap());
            assert_eq!(ordinary.to_bits(), expected.to_bits());
            // The original cubic has x=6t and y=12t(1-t). Over x=2..3,
            // its mean height is26/9, so the covered part of row3 is
            // 4-origin.y-26/9. This evaluation avoids the raster kernel.
            let original = 10.0 / 9.0 - origin.y;
            assert_eq!(quantize_unit(original), 127);
            let mut found = false;
            let mut cells = Cells::new(8, &mut crate::testing::bytes()).unwrap();
            raster::inspect(&shape, &mut cells, |y, x0, x1, value, arithmetic| {
                if y == 3 && x0 <= 2 && x1 > 2 {
                    found = true;
                    let error = ordinary + arithmetic + 8.0 * f64::EPSILON;
                    assert!((value.abs() - original).abs() <= error);
                }
            })
            .unwrap();
            assert!(found);
            let ready = certify(&edges, origin, Some(shape), &options)
                .unwrap()
                .unwrap();
            let (coverage, _) = crate::testing::raster(&options.area, Some(&ready), None);
            assert_eq!(coverage[3 * 8 + 2], 127);
            assert_eq!(coverage[4 * 8 + 2], if reversed { 0 } else { 255 });
        }
    }
}

#[test]
fn saturated_winding_keeps_original_control_and_nonfinite_uncertainty() {
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
    path.move_to(Point::new(2.25, 4.25));
    path.close();
    let edges = std::sync::Arc::new(
        GlyphEdges::new(&path.finish(), [1.0, 0.0, 0.0, 1.0], &limits).unwrap(),
    );
    let options = Prep {
        area: Area {
            region: DeviceRect {
                x0: 0,
                y0: 0,
                x1: 8,
                y1: 8,
            },
            band_height: 3,
        },
        limits: &limits,
        clip: None,
    };
    let shape = super::super::placed(&edges, Point::default(), &options)
        .unwrap()
        .unwrap();
    let mut cells = Cells::new(8, &mut crate::testing::bytes()).unwrap();
    let mut found = false;
    raster::inspect(&shape, &mut cells, |y, x0, x1, area, _| {
        if y == 4 && x0 <= 2 && x1 > 2 {
            found = true;
            assert!(area > 1.5);
        }
    })
    .unwrap();
    assert!(found);
    for uncertainty in [1.0 / 255.0, f64::NAN, f64::INFINITY] {
        let mut work = Meter::new(limits.raster_work, RASTER_WORK);
        let mut bytes = crate::testing::bytes();
        let pending = ambiguous(&shape, uncertainty, None, (&mut work, &mut bytes))
            .unwrap()
            .pixels;
        let pixel = *pending.iter().find(|p| p.x == 2 && p.y == 4).unwrap();
        assert_eq!(pixel.byte, 255);
        let source = Source::of(&edges, Point::default(), &mut work)
            .unwrap()
            .unwrap();
        assert_eq!(
            true_byte(source, pixel, &options, (&mut work, &mut bytes)).unwrap(),
            255
        );
    }
}

#[test]
fn local_proof_chords_keep_nonpoint_loops_distinct_from_points() {
    let limits = Limits::default();
    let area = Area {
        region: DeviceRect {
            x0: 0,
            y0: 0,
            x1: 1,
            y1: 1,
        },
        band_height: 1,
    };
    let mut clip_path = PathBuilder::new();
    clip_path.rect(0.75, 0.75, 0.25, 0.25);
    let clip_path = clip_path.finish();
    let clip = ClipShape::new(
        &[(&clip_path, FillRule::NonZero)],
        Transform::IDENTITY,
        &ShapeOptions {
            area,
            limits: &limits,
        },
    )
    .unwrap();
    let options = Prep {
        area,
        limits: &limits,
        clip: Some(&clip),
    };
    let start = Point::new(0.125, 0.125);
    for (control, expected) in [(Point::new(0.25, 0.25), 0), (start, 16)] {
        let mut path = PathBuilder::new();
        path.move_to(start);
        path.cubic_to(control, control, start).unwrap();
        path.close();
        let path = path.finish();
        let edges = GlyphEdges::new(&path, [1.0, 0.0, 0.0, 1.0], &limits).unwrap();
        let mut work = Meter::new(limits.raster_work, RASTER_WORK);
        let mut bytes = Meter::new(limits.render_bytes, RENDER_BYTES);
        let source = Source::of(&edges, Point::default(), &mut work)
            .unwrap()
            .unwrap();
        let (value, error) = interval(
            source,
            Pixel {
                x: 0,
                y: 0,
                byte: 0,
            },
            (&options, 0),
            (&mut work, &mut bytes),
        )
        .unwrap();
        // The out-and-back curve is nonpoint geometry outside the
        // partial clip. A genuine point still owns its whole floor cell.
        assert_eq!(quantize_unit(value - error), expected);
        assert_eq!(quantize_unit(value + error), expected);
    }
}

#[test]
fn analytic_integral_cancellation_cannot_skip_a_coverage_byte() {
    let limits = Limits::default();
    let m = 98_304.0;
    let mut path = PathBuilder::new();
    path.move_to(Point::default());
    path.cubic_to(
        Point::new(m / 6.0, m / 3.0),
        Point::new(m / 2.0, 2.0 * m / 3.0),
        Point::new(m, m),
    )
    .unwrap();
    path.line_to(Point::new(m + 2.0, m)).unwrap();
    path.line_to(Point::new(m + 2.0, 0.0)).unwrap();
    path.close();
    let path = path.finish();
    let edges = std::sync::Arc::new(GlyphEdges::new(&path, [1.0, 0.0, 0.0, 1.0], &limits).unwrap());
    assert!(edges.curved());
    let origin = Point::new(-98_302.997_063_444_05, 1.0 - m);
    let options = Prep {
        area: Area {
            region: DeviceRect {
                x0: 0,
                y0: 0,
                x1: 1,
                y1: 1,
            },
            band_height: 1,
        },
        limits: &limits,
        clip: None,
    };
    // Recover z=M(1-t) from x=0 using the original quadratic, then
    // integrate its pixel-local polynomial. This avoids H(tb)-H(ta)
    // and its large, nearly equal antiderivative values entirely.
    let u = origin.x + m;
    let z = 2.0 * u / (1.5 + (2.25 - 2.0 * u / m).sqrt());
    let exact = 1.0 - (u * z - 0.75 * z * z + z * z * z / (6.0 * m));
    assert!((exact - 0.664_705_581_461_998_8).abs() < 2e-15);
    assert_eq!(quantize_unit(exact), 169);
    let prepared = super::super::placed(&edges, origin, &options)
        .unwrap()
        .unwrap();
    let mut cells = Cells::new(1, &mut Meter::new(limits.render_bytes, RENDER_BYTES)).unwrap();
    raster::inspect(&prepared, &mut cells, |_, _, _, value, error| {
        assert!(
            (value - exact).abs() <= error,
            "primitive area {value} ±{error} excludes analytic {exact}"
        );
    })
    .unwrap();
    let mut work = Meter::new(limits.raster_work, RASTER_WORK);
    let mut bytes = Meter::new(limits.render_bytes, RENDER_BYTES);
    assert_eq!(
        ambiguous(&prepared, 0.0, None, (&mut work, &mut bytes))
            .unwrap()
            .pixels
            .len(),
        1,
    );
    match super::super::place(&edges, origin, &options) {
        Ok(Some(prepared)) => {
            raster::inspect(&prepared, &mut cells, |_, _, _, value, _| {
                assert_eq!(quantize_unit(value), 169);
            })
            .unwrap();
        }
        Err(error) => {
            assert!(matches!(error.kind, pagura_core::ErrorKind::Limit));
            assert!(matches!(
                error.message,
                "unresolved curve coverage interval" | "curve subdivision depth exceeded"
            ));
        }
        Ok(None) => panic!("the mark covers the pixel"),
    }
}

#[test]
fn replacement_releases_dead_builder_scratch_before_the_next_inspection() {
    let limits = Limits::default();
    let mut path = PathBuilder::new();
    let h = 3.0 * 0.500_001 / 255.0;
    for _ in 0..2 {
        path.move_to(Point::default());
        path.cubic_to(
            Point::new(1.0 / 3.0, 2.0 * h / 3.0),
            Point::new(2.0 / 3.0, 2.0 * h / 3.0),
            Point::new(1.0, 0.0),
        )
        .unwrap();
        path.close();
    }
    let edges = std::sync::Arc::new(
        GlyphEdges::polygon_for_test(&path.finish(), [1.0, 0.0, 0.0, 1.0], &limits).unwrap(),
    );
    let options = Prep {
        area: Area {
            region: DeviceRect {
                x0: 0,
                y0: 0,
                x1: 1,
                y1: 1,
            },
            band_height: 1,
        },
        limits: &limits,
        clip: None,
    };
    let shape = super::super::placed(&edges, Point::default(), &options)
        .unwrap()
        .unwrap();
    let mut work = Meter::new(limits.raster_work, RASTER_WORK);
    let source = Source::of(&edges, Point::default(), &mut work)
        .unwrap()
        .unwrap();
    let mut bytes = Meter::new(limits.render_bytes, RENDER_BYTES);
    bytes.charge(shape.retained_bytes()).unwrap();
    let shape = replacement(
        source,
        shape,
        &[[0.0, 0.0, 1.0, 1.0]],
        (&options, 0),
        Refinement::Whole,
        Meters {
            work: &mut work,
            bytes: &mut bytes,
        },
    )
    .unwrap()
    .shape
    .unwrap();
    assert_eq!(bytes.used(), shape.retained_bytes());
    let Body::Edges(body) = &shape.body else {
        panic!("replacement must be a polygon")
    };
    let mut needed = Meter::new(usize::MAX, RENDER_BYTES);
    Cells::charge(body.scan.1 - body.scan.0, &mut needed).unwrap();
    // One pending pixel must coexist with the cell buffer. Dead row
    // builder allocations must not consume this exact next-stage peak.
    let mut tight = Meter::new(
        bytes.used() + needed.used() + size_of::<Pixel>(),
        RENDER_BYTES,
    );
    tight.charge(bytes.used()).unwrap();
    let pending = ambiguous(&shape, AREA, None, (&mut work, &mut tight))
        .unwrap()
        .pixels;
    assert_eq!(pending.len(), 1);
    assert_eq!(tight.used(), shape.retained_bytes() + size_of::<Pixel>());
}

#[test]
fn canonical_clip_interpolation_cancellation_stays_inside_the_interval() {
    let limits = Limits::default();
    let n = 2.0_f64.powi(39);
    let u = 2.0_f64.powi(-13);
    let mut clip = PathBuilder::new();
    clip.move_to(Point::new(-n, -n));
    clip.line_to(Point::new(n, n + u)).unwrap();
    clip.line_to(Point::new(-n, n + 1.0)).unwrap();
    clip.close();
    let clip = clip.finish();
    let area = Area {
        region: DeviceRect {
            x0: 0,
            y0: 0,
            x1: 1,
            y1: 1,
        },
        band_height: 1,
    };
    let clip = ClipShape::new(
        &[(&clip, FillRule::NonZero)],
        Transform::IDENTITY,
        &ShapeOptions {
            area,
            limits: &limits,
        },
    )
    .unwrap();
    // This quadratic mark contains the whole unit pixel and has its
    // curved boundary outside it. Only the canonical clip can change
    // the pixel, so zero curve/localization error is not sufficient.
    let mut mark = PathBuilder::new();
    mark.move_to(Point::new(-2.0, -2.0));
    mark.cubic_to(
        Point::new(-2.0 / 3.0, -10.0 / 3.0),
        Point::new(2.0 / 3.0, -10.0 / 3.0),
        Point::new(2.0, -2.0),
    )
    .unwrap();
    mark.line_to(Point::new(2.0, 2.0)).unwrap();
    mark.line_to(Point::new(-2.0, 2.0)).unwrap();
    mark.close();
    let mark = mark.finish();
    let source = Source {
        outline: &mark,
        linear: Transform::IDENTITY,
        origin: Point::default(),
        points: &[],
        initial_area: AREA,
        proof: None,
    };
    let options = Prep {
        area,
        limits: &limits,
        clip: Some(&clip),
    };
    let mut work = Meter::new(limits.raster_work, RASTER_WORK);
    let mut bytes = Meter::new(limits.render_bytes, RENDER_BYTES);
    let (actual, error) = interval(
        source,
        Pixel {
            x: 0,
            y: 0,
            byte: 0,
        },
        (&options, 0),
        (&mut work, &mut bytes),
    )
    .unwrap();
    // The original stored line is x(y)=(y-u/2)/(1+u/(2N)). Integrate
    // from its x=0 crossing u/2 to y=1, independently of Line::x.
    let exact = (1.0 - u / 2.0).powi(2) / (2.0 * (1.0 + u / (2.0 * n)));
    assert_eq!(quantize_unit(exact), 127);
    assert!(
        (actual - exact).abs() <= error + 1e-15,
        "clip area{actual} ±{error} excludes analytic {exact}"
    );
    let edges = std::sync::Arc::new(GlyphEdges::new(&mark, [1.0, 0.0, 0.0, 1.0], &limits).unwrap());
    assert_eq!(
        super::super::place(&edges, Point::default(), &options)
            .unwrap_err()
            .message,
        "unresolved curve coverage interval",
    );
}

#[test]
fn canonical_source_line_cancellation_cannot_bypass_curve_intervals() {
    let limits = Limits::default();
    let n = 2.0_f64.powi(39);
    let u = 2.0_f64.powi(-13);
    let mut path = PathBuilder::new();
    path.move_to(Point::new(-n, -n));
    path.line_to(Point::new(n, n + u)).unwrap();
    path.line_to(Point::new(-n, n + 1.0)).unwrap();
    path.close();
    for _ in 0..2 {
        path.move_to(Point::new(3.0, 3.0));
        path.cubic_to(
            Point::new(3.0 + 1.0 / 3.0, 3.0 + 2.0 / 3.0),
            Point::new(3.0 + 2.0 / 3.0, 3.0 + 2.0 / 3.0),
            Point::new(4.0, 3.0),
        )
        .unwrap();
        path.close();
    }
    let path = path.finish();
    let edges = std::sync::Arc::new(GlyphEdges::new(&path, [1.0, 0.0, 0.0, 1.0], &limits).unwrap());
    assert!(!edges.curved());
    let area = Area {
        region: DeviceRect {
            x0: 0,
            y0: 0,
            x1: 1,
            y1: 1,
        },
        band_height: 1,
    };
    let options = Prep {
        area,
        limits: &limits,
        clip: None,
    };
    let source = Source {
        outline: &path,
        linear: Transform::IDENTITY,
        origin: Point::default(),
        points: &[],
        initial_area: AREA,
        proof: None,
    };
    let mut work = Meter::new(limits.raster_work, RASTER_WORK);
    let mut bytes = Meter::new(limits.render_bytes, RENDER_BYTES);
    let (actual, error) = interval(
        source,
        Pixel {
            x: 0,
            y: 0,
            byte: 0,
        },
        (&options, 0),
        (&mut work, &mut bytes),
    )
    .unwrap();
    let exact = (1.0 - u / 2.0).powi(2) / (2.0 * (1.0 + u / (2.0 * n)));
    assert_eq!(quantize_unit(exact), 127);
    assert!((actual - exact).abs() <= error + 1e-15);
    assert_eq!(
        super::super::place(&edges, Point::default(), &options)
            .unwrap_err()
            .message,
        "unresolved curve coverage interval",
    );
}

#[derive(Clone, Copy)]
enum Direction {
    Forward,
    Reverse,
}

fn local_proof_arch(path: &mut PathBuilder, at: Point, direction: Direction) {
    let p = |x, y| Point::new(at.x + x, at.y + y);
    if matches!(direction, Direction::Reverse) {
        path.move_to(p(0.0, 7.0));
        path.line_to(p(6.0, 7.0)).unwrap();
        path.line_to(p(6.0, 0.0)).unwrap();
        path.cubic_to(p(4.0, 4.0), p(2.0, 4.0), p(0.0, 0.0))
            .unwrap();
    } else {
        path.move_to(p(0.0, 0.0));
        path.cubic_to(p(2.0, 4.0), p(4.0, 4.0), p(6.0, 0.0))
            .unwrap();
        path.line_to(p(6.0, 7.0)).unwrap();
        path.line_to(p(0.0, 7.0)).unwrap();
    }
    path.close();
}

#[test]
fn local_leaf_enclosures_ignore_a_distant_opposite_lobe() {
    let limits = Limits::default();
    let mut path = PathBuilder::new();
    local_proof_arch(&mut path, Point::default(), Direction::Forward);
    local_proof_arch(&mut path, Point::new(1000.0, 0.0), Direction::Reverse);
    let edges = std::sync::Arc::new(
        GlyphEdges::new(&path.finish(), [1.0, 0.0, 0.0, 1.0], &limits).unwrap(),
    );
    let origin = Point::new(0.0, 11.0 / 18.0 + 1e-10);
    let options = Prep {
        area: Area {
            region: DeviceRect {
                x0: 0,
                y0: 0,
                x1: 8,
                y1: 10,
            },
            band_height: 3,
        },
        limits: &limits,
        clip: None,
    };
    let mut work = Meter::new(limits.raster_work, RASTER_WORK);
    let source = Source::of(&edges, origin, &mut work).unwrap().unwrap();
    let proof = source
        .proof
        .expect("opposite source signs require a polygon");
    let local = proof
        .error([2.0, 3.0, 3.0, 4.0], origin, &mut work)
        .unwrap();
    let distant = proof
        .error([20.0, 20.0, 21.0, 21.0], origin, &mut work)
        .unwrap();
    assert!(local < source.initial_area);
    assert!(distant <= (AREA * 0.5).next_up());

    let shape = super::super::placed(&edges, origin, &options)
        .unwrap()
        .unwrap();
    let placement = placement_error(source.outline, source.linear, origin, &shape);
    // x=6t, y=12t(1-t); its mean height over x=2..3 is26/9.
    // The opposite lobe is disjoint, so the original local area is unchanged.
    let exact = 10.0 / 9.0 - origin.y;
    assert_eq!(quantize_unit(exact), 127);
    let mut cells = Cells::new(8, &mut crate::testing::bytes()).unwrap();
    let mut found = false;
    raster::inspect(&shape, &mut cells, |y, x0, x1, area, arithmetic| {
        if y == 3 && x0 <= 2 && x1 > 2 {
            found = true;
            assert!((area - exact).abs() <= outward(local + placement + arithmetic));
        }
    })
    .unwrap();
    assert!(found);
    let mut bytes = crate::testing::bytes();
    let pending = ambiguous(
        &shape,
        source.initial_area + placement,
        Some(Local {
            proof,
            origin,
            other_error: placement,
        }),
        (&mut work, &mut bytes),
    )
    .unwrap()
    .pixels;
    assert!(pending.iter().any(|p| p.x == 2 && p.y == 3));
    assert!(!pending.iter().any(|p| p.x == 7 && p.y == 4));
    let ready = super::super::place(&edges, origin, &options)
        .unwrap()
        .unwrap();
    let (pixels, _) = crate::testing::raster(&options.area, Some(&ready), None);
    assert_eq!(pixels[3 * 8 + 2], 127);
}

#[test]
fn local_leaf_certificates_keep_cancellation_and_coincident_cells() {
    let limits = Limits::default();
    let mut path = PathBuilder::new();
    local_proof_arch(&mut path, Point::default(), Direction::Forward);
    local_proof_arch(&mut path, Point::default(), Direction::Reverse);
    for _ in 0..3 {
        path.move_to(Point::new(2.25, 4.25));
        path.close();
    }
    let edges = std::sync::Arc::new(
        GlyphEdges::new(&path.finish(), [1.0, 0.0, 0.0, 1.0], &limits).unwrap(),
    );
    let origin = Point::new(0.375, 0.25);
    let mut work = Meter::new(limits.raster_work, RASTER_WORK);
    assert!(
        Source::of(&edges, origin, &mut work)
            .unwrap()
            .unwrap()
            .proof
            .is_some()
    );
    for region in [
        DeviceRect {
            x0: 0,
            y0: 0,
            x1: 8,
            y1: 10,
        },
        DeviceRect {
            x0: 1,
            y0: 3,
            x1: 6,
            y1: 7,
        },
    ] {
        for band_height in [1, 3, 16, 32] {
            let options = Prep {
                area: Area {
                    region,
                    band_height,
                },
                limits: &limits,
                clip: None,
            };
            let shape = super::super::place(&edges, origin, &options)
                .unwrap()
                .unwrap();
            let (pixels, spent) = crate::testing::raster(&options.area, Some(&shape), None);
            assert!(spent <= shape.work());
            // Identical opposite contours cancel pointwise. The three
            // coincident canonical cells retain nonzero winding once.
            for y in region.y0..region.y1 {
                for x in region.x0..region.x1 {
                    let index = (y - region.y0) as usize * region.width() as usize
                        + (x - region.x0) as usize;
                    assert_eq!(pixels[index], if x == 2 && y == 4 { 255 } else { 0 });
                }
            }
        }
    }
}

#[test]
fn leaf_queries_round_a_cancellation_sized_origin_outward() {
    let limits = Limits::default();
    let anchor = 2.0_f64.powi(20);
    let origin = Point::new(-anchor + 2.0_f64.powi(-33), 0.0);
    let area = 1.0 / 64.0;
    let hull = Hull {
        bounds: [anchor + 1.0 - 1.0 / 128.0, 0.0, anchor + 1.0, 1.0],
        area,
    };
    // Its placed right edge is strictly inside pixel1, but an unrounded
    // inverse subtraction rounds the query's left edge to the hull's end.
    assert!(hull.bounds[2] + origin.x > 1.0);
    let mut bytes = crate::testing::bytes();
    let mut hulls = Vec::new();
    bytes.reserve(&mut hulls, 1).unwrap();
    hulls.push(hull);
    let mut work = Meter::new(limits.raster_work, RASTER_WORK);
    let proof = Proof::new(hulls, &mut bytes, &mut work).unwrap();
    let intersects = proof
        .error([1.0, 0.0, 2.0, 1.0], origin, &mut work)
        .unwrap();
    let disjoint = proof
        .error([2.0, 0.0, 3.0, 1.0], origin, &mut work)
        .unwrap();
    assert!(intersects >= area);
    assert!(disjoint <= (AREA * 0.5).next_up());
}

#[test]
fn successive_replacements_keep_the_previous_proof_charged_until_retirement() {
    let limits = Limits::default();
    let mut path = PathBuilder::new();
    local_proof_arch(&mut path, Point::default(), Direction::Forward);
    local_proof_arch(&mut path, Point::new(1000.0, 0.0), Direction::Reverse);
    let edges = std::sync::Arc::new(
        GlyphEdges::new(&path.finish(), [1.0, 0.0, 0.0, 1.0], &limits).unwrap(),
    );
    let origin = Point::new(0.0, 11.0 / 18.0 + 1e-10);
    let options = Prep {
        area: Area {
            region: DeviceRect {
                x0: 0,
                y0: 0,
                x1: 8,
                y1: 10,
            },
            band_height: 3,
        },
        limits: &limits,
        clip: None,
    };
    let mut work = Meter::new(limits.raster_work, RASTER_WORK);
    let source = Source::of(&edges, origin, &mut work).unwrap().unwrap();
    assert!(source.proof.is_some());
    let shape = super::super::placed(&edges, origin, &options)
        .unwrap()
        .unwrap();
    let mut bytes = Meter::new(limits.render_bytes, RENDER_BYTES);
    bytes.charge(shape.retained_bytes()).unwrap();
    let mut targets = Vec::new();
    bytes.reserve_capacity(&mut targets, 1).unwrap();
    targets.push([2.0, 3.0, 3.0, 4.0]);
    let target_bytes = targets.capacity() * size_of::<[f64; 4]>();
    let first = replacement(
        source,
        shape,
        &targets,
        (&options, 0),
        Refinement::Targeted,
        Meters {
            work: &mut work,
            bytes: &mut bytes,
        },
    )
    .unwrap();
    assert_eq!(first.area, source.initial_area);
    let first_shape = first.shape.unwrap();
    let first_proof = first.proof.unwrap();
    let first_proof_bytes = first_proof.retained_bytes();
    assert!(first_proof_bytes > 0);
    assert_eq!(
        bytes.used(),
        first_shape.retained_bytes() + target_bytes + first_proof_bytes,
    );
    let second = replacement(
        source,
        first_shape,
        &targets,
        (&options, 1),
        Refinement::Targeted,
        Meters {
            work: &mut work,
            bytes: &mut bytes,
        },
    )
    .unwrap();
    assert_eq!(second.area, source.initial_area);
    let second_shape = second.shape.unwrap();
    let second_proof = second.proof.unwrap();
    let second_proof_bytes = second_proof.retained_bytes();
    // The caller still owns the first proof throughout construction. A
    // replacement must not normalize that live owner out of the meter.
    let live = second_shape.retained_bytes() + target_bytes + second_proof_bytes;
    assert_eq!(bytes.used(), live + first_proof_bytes);
    drop(first_proof);
    bytes.refund(first_proof_bytes);
    assert_eq!(bytes.used(), live);
    let shape_bytes = second_shape.retained_bytes();
    drop(second_shape);
    bytes.refund(shape_bytes);
    drop(second_proof);
    bytes.refund(second_proof_bytes);
    drop(targets);
    bytes.refund(target_bytes);
    assert_eq!(bytes.used(), 0);
}

#[test]
fn owned_replacement_evidence_survives_inspection_and_keeps_point_band_bytes() {
    let limits = Limits::default();
    let mut path = PathBuilder::new();
    local_proof_arch(&mut path, Point::default(), Direction::Forward);
    local_proof_arch(&mut path, Point::new(1000.0, 0.0), Direction::Reverse);
    for _ in 0..2 {
        path.move_to(Point::new(7.25, 4.25));
        path.close();
    }
    let edges = std::sync::Arc::new(
        GlyphEdges::new(&path.finish(), [1.0, 0.0, 0.0, 1.0], &limits).unwrap(),
    );
    let origin = Point::new(0.0, 11.0 / 18.0 + 1e-10);
    let region = DeviceRect {
        x0: 0,
        y0: 0,
        x1: 10,
        y1: 10,
    };
    let options = Prep {
        area: Area {
            region,
            band_height: 3,
        },
        limits: &limits,
        clip: None,
    };
    let mut work = Meter::new(limits.raster_work, RASTER_WORK);
    let source = Source::of(&edges, origin, &mut work).unwrap().unwrap();
    let shape = super::super::placed(&edges, origin, &options)
        .unwrap()
        .unwrap();
    let mut bytes = Meter::new(limits.render_bytes, RENDER_BYTES);
    bytes.charge(shape.retained_bytes()).unwrap();
    let mut targets = Vec::new();
    bytes.reserve_capacity(&mut targets, 1).unwrap();
    targets.push([2.0, 3.0, 3.0, 4.0]);
    let target_bytes = targets.capacity() * size_of::<[f64; 4]>();
    let replaced = replacement(
        source,
        shape,
        &targets,
        (&options, 0),
        Refinement::Targeted,
        Meters {
            work: &mut work,
            bytes: &mut bytes,
        },
    )
    .unwrap();
    let shape = replaced.shape.unwrap();
    let proof = replaced.proof.unwrap();
    let live = shape.retained_bytes() + target_bytes + proof.retained_bytes();
    assert_eq!(bytes.used(), live);
    let placement = placement_error(source.outline, source.linear, origin, &shape);
    let local = Some(Local {
        proof: &proof,
        origin,
        other_error: placement,
    });
    let Body::Edges(body) = &shape.body else {
        panic!("a replacement uses canonical polygon geometry")
    };
    let mut needed = Meter::new(usize::MAX, RENDER_BYTES);
    Cells::charge(body.scan.1 - body.scan.0, &mut needed).unwrap();
    // The exact cell allocation fits, but the near-half pixel still needs
    // a pending record. Active proof capacity cannot pay for that record.
    let mut tight = Meter::new(live + needed.used(), RENDER_BYTES);
    tight.charge(live).unwrap();
    let error = ambiguous(
        &shape,
        replaced.area + placement,
        local,
        (&mut work, &mut tight),
    )
    .unwrap_err();
    assert_eq!(error.kind, pagura_core::ErrorKind::Limit);
    assert_eq!(error.message, RENDER_BYTES);
    assert_eq!(tight.used(), live);
    let pending = ambiguous(
        &shape,
        replaced.area + placement,
        local,
        (&mut work, &mut bytes),
    )
    .unwrap()
    .pixels;
    assert!(pending.iter().any(|p| p.x == 2 && p.y == 3));
    let pending_bytes = pending.capacity() * size_of::<Pixel>();
    assert_eq!(bytes.used(), live + pending_bytes);
    drop(pending);
    bytes.refund(pending_bytes);
    assert_eq!(bytes.used(), live);
    assert_point_band_pixels(&edges, origin, region, &limits);
}

fn assert_point_band_pixels(
    edges: &std::sync::Arc<GlyphEdges>,
    origin: Point,
    region: DeviceRect,
    limits: &Limits,
) {
    // x=6t, y=12t(1-t) has mean height26/9 over x=2..3.
    // The distant opposite lobe and the two point cells do not meet it.
    let exact = 10.0 / 9.0 - origin.y;
    assert_eq!(quantize_unit(exact), 127);
    let mut expected = None;
    for band_height in [1, 3, 16, 32] {
        let options = Prep {
            area: Area {
                region,
                band_height,
            },
            limits,
            clip: None,
        };
        let ready = super::super::place(edges, origin, &options)
            .unwrap()
            .unwrap();
        let (pixels, spent) = crate::testing::raster(&options.area, Some(&ready), None);
        assert!(spent <= ready.work());
        assert_eq!(pixels[3 * 10 + 2], quantize_unit(exact));
        assert_eq!(pixels[4 * 10 + 7], 255);
        assert_eq!(pixels[3 * 10 + 7], 0);
        if let Some(expected) = &expected {
            assert_eq!(&pixels, expected);
        } else {
            expected = Some(pixels);
        }
    }
}

fn original_reuse_fixture(limits: &Limits) -> (std::sync::Arc<GlyphEdges>, Prep<'_>) {
    let mut path = PathBuilder::new();
    local_proof_arch(&mut path, Point::default(), Direction::Forward);
    local_proof_arch(&mut path, Point::new(1000.0, 0.0), Direction::Reverse);
    let edges =
        std::sync::Arc::new(GlyphEdges::new(&path.finish(), [1.0, 0.0, 0.0, 1.0], limits).unwrap());
    let options = Prep {
        area: Area {
            region: DeviceRect {
                x0: 0,
                y0: 0,
                x1: 8,
                y1: 10,
            },
            band_height: 1,
        },
        limits,
        clip: None,
    };
    (edges, options)
}

fn prove_original_pixel(
    source: Source<'_>,
    pixel: Pixel,
    options: &Prep<'_>,
    Meters { work, bytes }: Meters<'_>,
) -> Vec<Pixel> {
    let mut pending = Vec::new();
    bytes.reserve_capacity(&mut pending, 1).unwrap();
    pending.push(pixel);
    let released = pending.capacity() * size_of::<Pixel>();
    let (mut targets, mut proven) = (Vec::new(), Vec::new());
    assert!(
        !collect_targets(
            source,
            pending,
            (&mut targets, &mut proven),
            options,
            Meters { work, bytes },
        )
        .unwrap()
    );
    bytes.refund(released);
    assert_eq!(targets, [] as [[f64; 4]; 0]);
    assert_eq!(proven.len(), 1);
    assert_eq!(proven[0].byte, pixel.byte);
    proven
}

#[test]
fn original_byte_reuse_still_detects_a_changed_wrong_candidate() {
    let limits = Limits::default();
    let (edges, options) = original_reuse_fixture(&limits);
    let origin = Point::new(0.0, 11.0 / 18.0 + 1e-10);
    let mut work = Meter::new(limits.raster_work, RASTER_WORK);
    let source = Source::of(&edges, origin, &mut work).unwrap().unwrap();
    let mut bytes = Meter::new(limits.render_bytes, RENDER_BYTES);
    // Integrating y=2x-x²/3 over x=2..3 gives the covered area
    // 10/9-origin.y, just below the half-level: its byte is127.
    let pixel = Pixel {
        x: 2,
        y: 3,
        byte: quantize_unit(10.0 / 9.0 - origin.y),
    };
    assert_eq!(pixel.byte, 127);
    let mut proven = prove_original_pixel(
        source,
        pixel,
        &options,
        Meters {
            work: &mut work,
            bytes: &mut bytes,
        },
    );
    assert!(work.used() > 64);
    // This budget cannot repeat the original-control proof. A newly
    // wrong candidate must still be detected from its successful byte.
    let mut hit_work = Meter::new(64, RASTER_WORK);
    let mut pending = Vec::new();
    bytes.reserve_capacity(&mut pending, 1).unwrap();
    // The earlier original-control certificate still detects this mismatch.
    pending.push(Pixel { byte: 0, ..pixel });
    let released = pending.capacity() * size_of::<Pixel>();
    let mut targets = Vec::new();
    assert!(
        collect_targets(
            source,
            pending,
            (&mut targets, &mut proven),
            &options,
            Meters {
                work: &mut hit_work,
                bytes: &mut bytes,
            },
        )
        .unwrap()
    );
    bytes.refund(released);
    assert_eq!(proven.len(), 1);
    assert_eq!(targets, [[2.0, 3.0, 3.0, 4.0]]);
    assert_eq!(
        bytes.used(),
        proven.capacity() * size_of::<Pixel>() + targets.capacity() * size_of::<[f64; 4]>(),
    );
}

#[test]
fn original_byte_certificates_do_not_survive_a_different_clip_or_call() {
    let limits = Limits::default();
    let (edges, options) = original_reuse_fixture(&limits);
    let origin = Point::new(0.0, 11.0 / 18.0 + 1e-10);
    let mut path = PathBuilder::new();
    path.rect(2.25, 0.0, 4.0, 10.0);
    let clip = ClipShape::new(
        &[(&path.finish(), FillRule::NonZero)],
        Transform::IDENTITY,
        &ShapeOptions {
            area: options.area,
            limits: &limits,
        },
    )
    .unwrap();
    for band_height in [1, 32] {
        for clipped in [false, true, false] {
            let options = Prep {
                area: Area {
                    band_height,
                    ..options.area
                },
                clip: clipped.then_some(&clip),
                ..options
            };
            let exact = if clipped {
                // Integral of 4-origin.y-2x+x²/3 over x=9/4..3.
                51.0 / 64.0 - 3.0 * origin.y / 4.0
            } else {
                10.0 / 9.0 - origin.y
            };
            let mut work = Meter::new(limits.raster_work, RASTER_WORK);
            let source = Source::of(&edges, origin, &mut work).unwrap().unwrap();
            let mut bytes = Meter::new(limits.render_bytes, RENDER_BYTES);
            let proven = prove_original_pixel(
                source,
                Pixel {
                    x: 2,
                    y: 3,
                    byte: quantize_unit(exact),
                },
                &options,
                Meters {
                    work: &mut work,
                    bytes: &mut bytes,
                },
            );
            assert_eq!(proven[0].byte, if clipped { 86 } else { 127 });
            let ready = super::super::place(&edges, origin, &options)
                .unwrap()
                .unwrap();
            let (pixels, spent) = crate::testing::raster(&options.area, Some(&ready), options.clip);
            assert!(spent <= ready.work());
            assert_eq!(pixels[3 * 8 + 2], quantize_unit(exact));
        }
    }
}

#[test]
fn original_byte_reuse_keeps_adjacent_work_and_byte_limits() {
    #[derive(Clone, Copy)]
    enum Budget {
        Measure,
        Work,
        Bytes,
    }
    let limits = Limits::default();
    let (edges, options) = original_reuse_fixture(&limits);
    let origin = Point::new(0.0, 11.0 / 18.0 + 1e-10);
    let mut work = Meter::new(limits.raster_work, RASTER_WORK);
    let source = Source::of(&edges, origin, &mut work).unwrap().unwrap();
    let mut bytes = Meter::new(limits.render_bytes, RENDER_BYTES);
    let pixel = Pixel {
        x: 2,
        y: 3,
        byte: 127,
    };
    let proven = prove_original_pixel(
        source,
        pixel,
        &options,
        Meters {
            work: &mut work,
            bytes: &mut bytes,
        },
    );
    let mut boundary = None;
    for budget in [Budget::Measure, Budget::Work, Budget::Bytes] {
        for offset in [0, 1] {
            if matches!(budget, Budget::Measure) && offset != 0 {
                continue;
            }
            let (work_limit, byte_limit) = boundary.map_or(
                (limits.raster_work, limits.render_bytes),
                |(needed_work, needed_bytes): (usize, usize)| match budget {
                    Budget::Work => (needed_work - offset, limits.render_bytes),
                    Budget::Bytes => (limits.raster_work, needed_bytes - offset),
                    Budget::Measure => (limits.raster_work, limits.render_bytes),
                },
            );
            let mut work = Meter::new(work_limit, RASTER_WORK);
            let mut bytes = Meter::new(byte_limit, RENDER_BYTES);
            let mut proven = proven.clone();
            bytes.charge_items::<Pixel>(proven.capacity()).unwrap();
            let mut pending = Vec::new();
            bytes.reserve_capacity(&mut pending, 1).unwrap();
            pending.push(Pixel { byte: 0, ..pixel });
            let mut targets = Vec::new();
            let result = collect_targets(
                source,
                pending,
                (&mut targets, &mut proven),
                &options,
                Meters {
                    work: &mut work,
                    bytes: &mut bytes,
                },
            );
            if matches!(budget, Budget::Measure) {
                assert!(result.unwrap());
                boundary = Some((work.used(), bytes.used()));
            } else if offset == 0 {
                assert!(result.unwrap());
                assert_eq!(targets, [[2.0, 3.0, 3.0, 4.0]]);
            } else {
                let error = result.unwrap_err();
                assert_eq!(error.kind, pagura_core::ErrorKind::Limit);
                assert_eq!(
                    error.message,
                    match budget {
                        Budget::Work => RASTER_WORK,
                        Budget::Bytes => RENDER_BYTES,
                        Budget::Measure => unreachable!(),
                    },
                );
                assert_eq!(targets, [] as [[f64; 4]; 0]);
            }
            assert_eq!(proven.len(), 1);
            assert_eq!(proven[0].byte, 127);
        }
    }
}

fn complete_rows_path() -> Path {
    complete_rows_builder().finish()
}

fn complete_rows_builder() -> PathBuilder {
    let mut path = PathBuilder::new();
    path.move_to(Point::default());
    path.cubic_to(
        Point::new(2.0, 4.0),
        Point::new(4.0, 4.0),
        Point::new(6.0, 0.0),
    )
    .unwrap();
    // These original endpoints must remain in every flattened replacement.
    // They supply a byte/work floor without relying on subdivision density.
    for i in 1..=64 {
        path.line_to(Point::new(6.0, 7.0 * f64::from(i) / 64.0))
            .unwrap();
    }
    path.line_to(Point::new(0.0, 7.0)).unwrap();
    for i in (0..64).rev() {
        path.line_to(Point::new(0.0, 7.0 * f64::from(i) / 64.0))
            .unwrap();
    }
    path.close();
    path
}

fn complete_rows_options(limits: &Limits, band_height: u32) -> Prep<'_> {
    Prep {
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
        clip: None,
    }
}

fn complete_rows_shape(path: &Path, origin: Point, options: &Prep<'_>) -> Shape {
    let mut shape = complete_rows_bare_shape(path, origin, options);
    // A real paid reservation with ample unused work isolates owner admission
    // from the materiality question; every replay still checks its exact tally.
    shape.add_work(1_000_000);
    shape
}

fn complete_rows_bare_shape(path: &Path, origin: Point, options: &Prep<'_>) -> Shape {
    Shape::fill(
        path,
        Transform {
            e: origin.x,
            f: origin.y,
            ..Transform::IDENTITY
        },
        FillRule::NonZero,
        &ShapeOptions {
            area: options.area,
            limits: options.limits,
        },
    )
    .unwrap()
    .unwrap()
}

fn complete_rows_source(path: &Path, origin: Point) -> Source<'_> {
    Source {
        outline: path,
        linear: Transform::IDENTITY,
        origin,
        points: &[],
        initial_area: AREA,
        proof: None,
    }
}

fn complete_rows_surplus(shape: &Shape) -> usize {
    let mut cells = Cells::new(scan_width(shape), &mut crate::testing::bytes()).unwrap();
    raster::inspect(shape, &mut cells, |_, _, _, _, _| {})
        .unwrap()
        .surplus(shape.work())
        .unwrap()
}

fn complete_rows_run(shape: &Shape, area: &Area) -> (Vec<u8>, usize) {
    let mut bytes = crate::testing::bytes();
    let mut raster = crate::BandRasterizer::new(*area, &mut bytes).unwrap();
    let mut band = crate::BandCoverage::with_capacity(shape.capacity(), &mut bytes).unwrap();
    let retained = band.retained_bytes();
    let width = area.region.width() as usize;
    let mut values = vec![0; width * area.region.height() as usize];
    for index in shape.bands(area) {
        assert_eq!(raster.rasterize_run(&[shape], None, index, &mut band), 1);
        assert_eq!(band.retained_bytes(), retained);
        for row in band.rows() {
            for span in row.spans() {
                let (x, cells) = match span {
                    crate::Span::Full { x, len } => (x, vec![255; len as usize]),
                    crate::Span::Partial { x, coverage } => (x, coverage.to_vec()),
                };
                let start =
                    (row.y - area.region.y0) as usize * width + (x - area.region.x0) as usize;
                values[start..start + cells.len()].copy_from_slice(&cells);
            }
        }
    }
    (values, raster.work_done())
}

#[test]
fn complete_rows_replay_original_bytes_and_keep_bounds_across_bands() {
    let limits = Limits::default();
    let path = complete_rows_path();
    let origin = Point::new(1.25, 103.0 / 144.0 + 1e-10);
    for band_height in [1, 3, 16, 32] {
        let options = complete_rows_options(&limits, band_height);
        let shape = complete_rows_shape(&path, origin, &options);
        let bounds = shape.bounds();
        let source = complete_rows_source(&path, origin);
        let mut work = Meter::new(limits.raster_work, RASTER_WORK);
        let mut bytes = Meter::new(limits.render_bytes, RENDER_BYTES);
        bytes.charge(shape.retained_bytes()).unwrap();
        let (mut targets, mut proven) = (Vec::new(), Vec::new());
        let collected = ambiguous(&shape, f64::INFINITY, None, (&mut work, &mut bytes))
            .unwrap()
            .collect(
                source,
                &options,
                (&mut targets, &mut proven),
                shape.retained_bytes(),
                Meters {
                    work: &mut work,
                    bytes: &mut bytes,
                },
            )
            .unwrap();
        assert!(collected.wrong);
        let mut expected = vec![0; 12 * 12];
        for pixel in &proven {
            expected[pixel.y as usize * 12 + pixel.x as usize] = pixel.byte;
        }
        // Over local x=7/4..11/4 the original arch's mean is401/144.
        // Its row3 coverage is175/144-origin.y, just below half a pixel.
        assert_eq!(expected[3 * 12 + 3], 127);
        let spent = work.used();
        let mut ready = rows::replace(
            source,
            shape,
            &options,
            &proven,
            collected.surplus,
            &mut bytes,
        );
        assert!(matches!(&ready.body, Body::Certified(_)));
        assert_eq!(ready.bounds(), bounds);
        assert_eq!(work.used(), spent, "the child never charges the parent");
        assert_eq!(
            bytes.used(),
            ready.retained_bytes()
                + targets.capacity() * size_of::<[f64; 4]>()
                + proven.capacity() * size_of::<Pixel>(),
        );
        ready.add_work(super::super::merged(bounds));
        let (actual, executed) = crate::testing::raster(&options.area, Some(&ready), None);
        assert_eq!(actual, expected);
        assert!(executed <= ready.work());
        let (gathered, executed) = complete_rows_run(&ready, &options.area);
        assert_eq!(gathered, actual);
        assert!(executed <= ready.work());
    }
}

#[test]
fn complete_rows_keep_canonical_point_floors_and_a_containing_clip() {
    struct Case {
        x: f64,
        band_height: u32,
        point_x: usize,
    }
    let limits = Limits::default();
    let mut path = complete_rows_builder();
    path.move_to(Point::new(7.75, 1.25));
    path.close();
    // Keep the floor-crossing point inside a separately supported bbox.
    // Its outward original MoveTo enclosure may cross8 even when the
    // literal placed point is just below8 and its canonical cell ends8.
    path.move_to(Point::new(9.25, 4.25));
    path.close();
    let edges = std::sync::Arc::new(
        GlyphEdges::new(&path.finish(), [1.0, 0.0, 0.0, 1.0], &limits).unwrap(),
    );
    for case in [
        Case {
            x: 0.25,
            band_height: 1,
            point_x: 8,
        },
        Case {
            x: 0.25 - 2.0_f64.powi(-49),
            band_height: 16,
            point_x: 7,
        },
    ] {
        let origin = Point::new(case.x, 0.25);
        let options = complete_rows_options(&limits, case.band_height);
        let mut rectangle = PathBuilder::new();
        rectangle.rect(0.0, 0.0, 12.0, 12.0);
        let clip = ClipShape::new(
            &[(&rectangle.finish(), FillRule::NonZero)],
            Transform::IDENTITY,
            &ShapeOptions {
                area: options.area,
                limits: &limits,
            },
        )
        .unwrap();
        let options = Prep {
            clip: Some(&clip),
            ..options
        };
        let mut shape = super::super::placed(&edges, origin, &options)
            .unwrap()
            .unwrap();
        shape.add_work(1_000_000);
        let mut work = Meter::new(limits.raster_work, RASTER_WORK);
        let source = Source::of(&edges, origin, &mut work).unwrap().unwrap();
        assert_eq!(source.points.len(), 2);
        assert!(support_contained(source, Some(&shape), &options, &mut work).unwrap());
        let mut bytes = crate::testing::bytes();
        bytes.charge(shape.retained_bytes()).unwrap();
        let (mut targets, mut proven) = (Vec::new(), Vec::new());
        let collected = ambiguous(&shape, f64::INFINITY, None, (&mut work, &mut bytes))
            .unwrap()
            .collect(
                source,
                &options,
                (&mut targets, &mut proven),
                shape.retained_bytes(),
                Meters {
                    work: &mut work,
                    bytes: &mut bytes,
                },
            )
            .unwrap();
        let mut expected = vec![0; 12 * 12];
        for pixel in &proven {
            expected[pixel.y as usize * 12 + pixel.x as usize] = pixel.byte;
        }
        assert_eq!(expected[12 + case.point_x], 255);
        assert_eq!(expected[4 * 12 + 9], 255);
        let ready = rows::replace(
            source,
            shape,
            &options,
            &proven,
            collected.surplus,
            &mut bytes,
        );
        assert!(matches!(&ready.body, Body::Certified(_)));
        assert_eq!(
            crate::testing::raster(&options.area, Some(&ready), Some(&clip)).0,
            expected
        );
    }
}

#[test]
fn complete_rows_declines_restore_parent_owners_work_and_bytes() {
    let limits = Limits::default();
    let path = complete_rows_path();
    let origin = Point::new(1.25, 0.25);
    let options = complete_rows_options(&limits, 3);
    for remaining in [0, limits.render_bytes] {
        let shape = complete_rows_shape(&path, origin, &options);
        let retained = shape.retained_bytes();
        let work = shape.work();
        let expected = crate::testing::raster(&options.area, Some(&shape), None).0;
        let mut bytes = Meter::new(retained + remaining, RENDER_BYTES);
        bytes.charge(retained).unwrap();
        let before = bytes.clone();
        let surplus = if remaining == 0 {
            complete_rows_surplus(&shape)
        } else {
            0
        };
        let ready = rows::replace(
            complete_rows_source(&path, origin),
            shape,
            &options,
            &[],
            Some(surplus),
            &mut bytes,
        );
        assert!(!matches!(&ready.body, Body::Certified(_)));
        assert_eq!((ready.retained_bytes(), ready.work()), (retained, work));
        assert_eq!(bytes, before);
        assert_eq!(
            crate::testing::raster(&options.area, Some(&ready), None).0,
            expected
        );
    }
}

#[test]
fn complete_rows_replay_and_encoding_exhaustion_remain_optional() {
    let limits = Limits::default();
    let path = complete_rows_path();
    let origin = Point::new(1.25, 0.25);
    let options = complete_rows_options(&limits, 3);
    let shape = complete_rows_shape(&path, origin, &options);
    let source = complete_rows_source(&path, origin);
    let surplus = complete_rows_surplus(&shape);
    let mut work = Meter::new(surplus, RASTER_WORK);
    let mut bytes = crate::testing::bytes();
    let ready = rows::attempt(
        source,
        &shape,
        &options,
        &[],
        surplus,
        Meters {
            work: &mut work,
            bytes: &mut bytes,
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(bytes.used(), ready.heap_bytes());
    let required = work.used();
    drop(ready);
    for bound in [shape.work() - surplus - 1, required - 1] {
        let mut work = Meter::new(bound, RASTER_WORK);
        let mut bytes = crate::testing::bytes();
        let result = rows::attempt(
            source,
            &shape,
            &options,
            &[],
            surplus,
            Meters {
                work: &mut work,
                bytes: &mut bytes,
            },
        );
        assert_eq!(result.unwrap_err().kind, pagura_core::ErrorKind::Limit);
        assert!(work.used() <= bound);
    }
    assert!(!matches!(&shape.body, Body::Certified(_)));
}

#[test]
fn complete_rows_cell_initialization_needs_its_own_unused_work() {
    let limits = Limits::default();
    let path = complete_rows_path();
    let origin = Point::new(1.25, 0.25);
    let options = complete_rows_options(&limits, 3);
    let mut shape = complete_rows_bare_shape(&path, origin, &options);
    let source = complete_rows_source(&path, origin);
    let mut cells = Cells::new(scan_width(&shape), &mut crate::testing::bytes()).unwrap();
    let observed = raster::inspect(&shape, &mut cells, |_, _, _, _, _| {}).unwrap();
    drop(cells);
    let replay = usize::MAX - observed.surplus(usize::MAX).unwrap();
    let mut setup = Meter::new(usize::MAX, RASTER_WORK);
    rows::floor(source, &shape, &options, &mut setup)
        .unwrap()
        .unwrap();
    let mut initialized = Meter::new(usize::MAX, RENDER_BYTES);
    Cells::charge(scan_width(&shape), &mut initialized).unwrap();
    let loan = replay + 16 + setup.used() + initialized.used() - 1;
    let allowance = replay + loan;
    assert!(allowance >= shape.work());
    shape.add_work(allowance - shape.work());
    assert_eq!(complete_rows_surplus(&shape), loan);
    let retained = shape.retained_bytes();
    let work = shape.work();
    let mut bytes = crate::testing::bytes();
    bytes.charge(retained).unwrap();
    let before = bytes.clone();
    let ready = rows::replace(source, shape, &options, &[], Some(loan), &mut bytes);
    assert!(!matches!(&ready.body, Body::Certified(_)));
    assert_eq!((ready.retained_bytes(), ready.work()), (retained, work));
    assert_eq!(bytes, before);
}

#[test]
fn complete_rows_reject_missing_borders_zero_results_and_unvisited_proofs() {
    let limits = Limits::default();
    let path = complete_rows_path();
    let origin = Point::new(1.25, 0.25);
    let options = complete_rows_options(&limits, 3);
    let shape = complete_rows_shape(&path, origin, &options);
    let bounds = shape.bounds();
    let source = complete_rows_source(&path, origin);
    let surplus = complete_rows_surplus(&shape);
    let mut zero = Vec::new();
    for y in bounds.y0..bounds.y1 {
        for x in bounds.x0..bounds.x1 {
            zero.push(Pixel { x, y, byte: 0 });
        }
    }
    let left: Vec<_> = zero.iter().copied().filter(|p| p.x == bounds.x0).collect();
    let outside = [Pixel {
        x: bounds.x0,
        y: bounds.y1,
        byte: 255,
    }];
    for proven in [zero.as_slice(), left.as_slice(), outside.as_slice()] {
        assert!(
            rows::prepare(
                source,
                &shape,
                &options,
                proven,
                surplus,
                limits.render_bytes
            )
            .is_none()
        );
    }
}

#[test]
fn complete_rows_floor_keeps_clips_points_and_right_boundary_witnesses() {
    let limits = Limits::default();
    let path = complete_rows_path();
    let origin = Point::new(1.25, 0.25);
    let options = complete_rows_options(&limits, 3);
    let shape = complete_rows_shape(&path, origin, &options);
    let source = complete_rows_source(&path, origin);
    let floor = rows::floor(source, &shape, &options, &mut crate::testing::bytes())
        .unwrap()
        .unwrap();
    assert_eq!(floor.bytes, 128 * size_of::<Line>());
    assert!(floor.work >= 24 * 128);
    // A canonical floor corner, not the untranslated point's coordinate,
    // extends outside the existing scan and must decline.
    let points = [Point::new(7.75, 1.0)];
    assert!(
        rows::floor(
            Source {
                points: &points,
                ..source
            },
            &shape,
            &options,
            &mut crate::testing::bytes()
        )
        .unwrap()
        .is_none()
    );
    let mut clip_path = PathBuilder::new();
    clip_path.rect(2.0, 0.0, 10.0, 12.0);
    let clip = ClipShape::new(
        &[(&clip_path.finish(), FillRule::NonZero)],
        Transform::IDENTITY,
        &ShapeOptions {
            area: options.area,
            limits: &limits,
        },
    )
    .unwrap();
    assert!(
        rows::floor(
            source,
            &shape,
            &Prep {
                clip: Some(&clip),
                ..options
            },
            &mut crate::testing::bytes()
        )
        .unwrap()
        .is_none()
    );

    let mut retraced = PathBuilder::new();
    retraced.move_to(Point::new(12.0, 1.25));
    for _ in 0..64 {
        retraced.line_to(Point::new(12.0, 1.75)).unwrap();
        retraced.line_to(Point::new(12.0, 1.25)).unwrap();
    }
    retraced.close();
    let retraced = retraced.finish();
    let mut point_box = PathBuilder::new();
    point_box.rect(11.0, 1.0, 1.0, 1.0);
    let point_box = complete_rows_shape(&point_box.finish(), Point::default(), &options);
    let points = [Point::new(11.25, 1.25)];
    let source = Source {
        points: &points,
        ..complete_rows_source(&retraced, Point::default())
    };
    assert!(
        support_contained(
            source,
            Some(&point_box),
            &options,
            &mut crate::testing::bytes()
        )
        .unwrap()
    );
    assert!(
        rows::floor(source, &point_box, &options, &mut crate::testing::bytes())
            .unwrap()
            .is_none()
    );
}

#[test]
fn complete_rows_floor_counts_numeric_endpoint_height_not_bits() {
    let mut flat = PathBuilder::new();
    flat.move_to(Point::new(1.0, -0.0));
    flat.line_to(Point::new(2.0, 0.0)).unwrap();
    let flat = flat.finish();
    let source = Source {
        linear: Transform::new(1.0, -0.0, -0.0, 1.0, 0.0, -0.0),
        ..complete_rows_source(&flat, Point::new(0.0, -0.0))
    };
    let a = Point::new(1.0, -0.0) * source.linear + source.origin;
    let b = Point::new(2.0, 0.0) * source.linear + source.origin;
    assert_ne!(a.y.to_bits(), b.y.to_bits());
    assert!(
        rows::witness(source, &mut crate::testing::bytes())
            .unwrap()
            .is_none()
    );
    let mut tiny = PathBuilder::new();
    tiny.move_to(Point::new(1.0, 1.0));
    tiny.line_to(Point::new(2.0, 1.0_f64.next_up())).unwrap();
    let tiny = tiny.finish();
    let origin = Point::new(0.0, 100_000_000.0);
    assert_eq!(
        (origin.y + 1.0).to_bits(),
        (origin.y + 1.0_f64.next_up()).to_bits()
    );
    assert!(
        rows::witness(
            complete_rows_source(&tiny, origin),
            &mut crate::testing::bytes()
        )
        .unwrap()
        .is_none()
    );
}

#[test]
fn complete_rows_leave_sources_without_original_curves_unchanged() {
    let limits = Limits::default();
    let options = complete_rows_options(&limits, 3);
    let mut path = PathBuilder::new();
    path.move_to(Point::new(1.25, 1.25));
    path.cubic_to(
        Point::new(2.25, 5.25),
        Point::new(4.25, 2.25),
        Point::new(5.25, 5.25),
    )
    .unwrap();
    path.line_to(Point::new(1.25, 5.25)).unwrap();
    path.close();
    let path = path.finish();
    assert!(!curve::quadratic_outline(&path, Transform::IDENTITY));
    let edges = std::sync::Arc::new(GlyphEdges::new(&path, [1.0, 0.0, 0.0, 1.0], &limits).unwrap());
    assert!(matches!(&edges.kind, Kind::Polygon { source: None, .. }));
    assert!(
        Source::of(&edges, Point::default(), &mut crate::testing::bytes())
            .unwrap()
            .is_none()
    );
    let shape = super::super::placed(&edges, Point::default(), &options)
        .unwrap()
        .unwrap();
    let expected = crate::testing::raster(&options.area, Some(&shape), None).0;
    let before = (shape.retained_bytes(), shape.work());
    let ready = certify(&edges, Point::default(), Some(shape), &options)
        .unwrap()
        .unwrap();
    assert!(!matches!(&ready.body, Body::Certified(_)));
    assert_eq!((ready.retained_bytes(), ready.work()), before);
    assert_eq!(
        crate::testing::raster(&options.area, Some(&ready), None).0,
        expected
    );
}
