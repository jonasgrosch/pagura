use super::*;

#[test]
fn round_pen_points_are_bounded_before_large_subdivision_allocates() {
    let mut out = PathBuilder::new();
    let mut bytes = Meter::new(128, RENDER_BYTES);
    let pen = Pen {
        radius: 1e8,
        tolerance: TOLERANCE,
    };
    let error = circle(
        &mut out,
        Point::default(),
        pen,
        &Limits::default(),
        &mut bytes,
    )
    .unwrap_err();
    assert_eq!(error.message, RENDER_BYTES);
    assert_eq!((out.len(), out.heap_bytes(), bytes.used()), (0, 0, 0));
}

#[test]
fn a_fine_dash_pattern_stops_at_the_byte_budget() {
    let mut bytes = Meter::new(2048, RENDER_BYTES);
    let mut points = Vec::new();
    bytes.reserve(&mut points, 2).unwrap();
    points.extend([Point::default(), Point::new(1e8, 0.0)]);
    let stroke = Stroke {
        dash_array: vec![1e-7, 1e-7],
        ..Stroke::default()
    };
    let error = dash(points, false, &stroke, &Limits::default(), &mut bytes)
        .err()
        .unwrap();
    assert_eq!(error.message, RENDER_BYTES);
    assert_eq!(
        bytes.used(),
        0,
        "the input and refused dashes were released"
    );
}

#[test]
fn an_outline_releases_scratch_but_keeps_its_output_charged() {
    let mut path = PathBuilder::new();
    path.polyline([
        Point::new(0.0, 0.0),
        Point::new(3.0, 2.0),
        Point::new(6.0, 0.0),
    ]);
    let path = path.finish();
    let mut bytes = crate::testing::bytes();
    bytes.charge(37).unwrap();
    let (outline, _) = outline(
        &path,
        Transform::IDENTITY,
        &Stroke {
            cap: LineCap::Round,
            join: LineJoin::Round,
            ..Stroke::default()
        },
        &Limits::default(),
        &mut bytes,
    )
    .unwrap();
    assert!(!outline.is_empty());
    assert_eq!(bytes.used(), 37 + outline.retained_bytes());
}

#[test]
fn table_55_run_sequences() {
    // Explicit on intervals derived from Table 55, independent of dash().
    type Example<'a> = (&'a [f64], f64, f64, &'a [(f64, f64)]);
    let examples: &[Example<'_>] = &[
        (&[], 0.0, 12.0, &[(0.0, 12.0)]),
        (&[3.0], 0.0, 12.0, &[(0.0, 3.0), (6.0, 9.0)]),
        (&[2.0], 1.0, 9.0, &[(0.0, 1.0), (3.0, 5.0), (7.0, 9.0)]),
        (&[2.0, 1.0], 0.0, 8.0, &[(0.0, 2.0), (3.0, 5.0), (6.0, 8.0)]),
        (&[3.0, 5.0], 6.0, 18.0, &[(2.0, 5.0), (10.0, 13.0)]),
        (
            &[2.0, 3.0],
            11.0,
            16.0,
            &[(0.0, 1.0), (4.0, 6.0), (9.0, 11.0), (14.0, 16.0)],
        ),
        (
            &[2.0, 1.0, 3.0],
            0.0,
            14.0,
            &[(0.0, 2.0), (3.0, 6.0), (8.0, 9.0), (12.0, 14.0)],
        ),
        (
            &[2.0, 1.0, 3.0],
            -2.0,
            14.0,
            &[(2.0, 4.0), (5.0, 8.0), (10.0, 11.0)],
        ),
    ];
    for &(array, phase, length, expected) in examples {
        let stroke = Stroke {
            dash_array: array.to_vec(),
            dash_phase: phase,
            ..Stroke::default()
        };
        let runs = dash(
            vec![Point::new(0.0, 0.0), Point::new(length, 0.0)],
            false,
            &stroke,
            &Limits::default(),
            &mut crate::testing::bytes(),
        )
        .unwrap();
        let actual: Vec<_> = runs
            .iter()
            .map(|run| (run.points.first().unwrap().x, run.points.last().unwrap().x))
            .collect();
        assert_eq!(actual, expected, "{array:?} {phase}");
    }
}

#[test]
fn mapped_pieces_are_the_polygon_of_their_path() {
    let limits = Limits::default();
    let zigzag = [(2.0, 2.5), (14.0, 4.0), (5.0, 12.0), (21.0, 20.5)];
    let mut paths = Vec::new();
    for closed in [false, true] {
        let mut path = PathBuilder::new();
        path.polyline(zigzag.iter().map(|&(x, y)| Point::new(x, y)));
        if closed {
            path.close();
        }
        paths.push(path.finish());
    }
    let mut dot = PathBuilder::new();
    dot.polyline([Point::new(7.0, 7.0), Point::new(7.0, 7.0)]);
    paths.push(dot.finish());
    for path in &paths {
        for width in [0.0, 0.6, 3.5] {
            for (cap, join) in [
                (LineCap::Butt, LineJoin::Miter),
                (LineCap::Round, LineJoin::Round),
                (LineCap::ProjectingSquare, LineJoin::Bevel),
            ] {
                let stroke = Stroke {
                    width,
                    cap,
                    join,
                    ..Stroke::default()
                };
                for transform in [
                    Transform::IDENTITY,
                    Transform::scale(1.0, 0.6).then(Transform::translate(0.3, 4.1)),
                ] {
                    let options = (Tiling::Unproven, TOLERANCE);
                    let mut bytes = crate::testing::bytes();
                    let (outline, pen, tiling) =
                        pieces(path, transform, &stroke, &limits, options, &mut bytes).unwrap();
                    let expected = Polygon::from_lines(&outline, pen, &limits, &mut bytes)
                        .unwrap()
                        .unwrap();
                    let (polygon, mapped) =
                        polygon_pieces(path, transform, &stroke, &limits, options, &mut bytes)
                            .unwrap();
                    assert_eq!(mapped, tiling);
                    assert_eq!(polygon.points, expected.points, "{stroke:?} {transform:?}");
                    assert_eq!(polygon.starts, expected.starts);
                    assert_eq!(polygon.points_cells, expected.points_cells);
                }
            }
        }
    }
}

#[test]
fn adjusted_pieces_cover_the_fitted_components() {
    use crate::{Adjustment, Area, FillRule, Shape, ShapeOptions};
    let limits = Limits::default();
    let options = ShapeOptions {
        area: Area {
            region: crate::testing::window(40, 40),
            band_height: 16,
        },
        limits: &limits,
    };
    let coverage = |shape: Option<Shape>| match shape {
        Some(shape) => crate::testing::raster(&options.area, Some(&shape), None).0,
        None => vec![0; 40 * 40],
    };
    let paths = adjusted_paths();
    let mut compared = 0;
    for (path, orthogonal) in &paths {
        for width in [0.0, 0.4, 1.0, 2.6] {
            for (cap, join, dash_array) in [
                (LineCap::Butt, LineJoin::Miter, vec![]),
                (LineCap::Round, LineJoin::Round, vec![]),
                (LineCap::ProjectingSquare, LineJoin::Bevel, vec![]),
                (LineCap::Round, LineJoin::Miter, vec![3.0, 1.5]),
                (LineCap::Butt, LineJoin::Round, vec![0.0, 2.0]),
            ] {
                let stroke = Stroke {
                    width,
                    cap,
                    join,
                    dash_array,
                    ..Stroke::default()
                };
                for transform in [
                    Transform::IDENTITY,
                    Transform::scale(1.0, 0.6).then(Transform::translate(0.3, 4.1)),
                    Transform::new(0.8, 0.6, -0.6, 0.8, 9.0, 2.0),
                ] {
                    let pieces = Shape::stroke(
                        path,
                        &stroke,
                        Transform::IDENTITY,
                        transform,
                        Adjustment::Adjusted,
                        &options,
                    )
                    .unwrap();
                    let (outline, pen) =
                        adjusted_stroke_outline(path, transform, &stroke, &limits).unwrap();
                    let components =
                        Shape::fill(&outline, pen, FillRule::NonZero, &options).unwrap();
                    let (pieces, components) = (coverage(pieces), coverage(components));
                    for (i, (&a, &b)) in pieces.iter().zip(&components).enumerate() {
                        assert!(
                            a.abs_diff(b) <= 1,
                            "pixel {i}: {a} against {b}, {stroke:?} {transform:?}"
                        );
                    }
                    compared += 1;
                    // Undashed axis-aligned outlines tile by construction.
                    if *orthogonal
                        && transform.b == 0.0
                        && stroke.dash_array.is_empty()
                        && cap != LineCap::Round
                    {
                        let mut bytes = crate::testing::bytes();
                        let (_, tiling) = adjusted_polygon_pieces(
                            path,
                            transform,
                            &stroke,
                            &limits,
                            Tiling::Unproven,
                            &mut bytes,
                        )
                        .unwrap();
                        assert_eq!(tiling, Tiling::Proven, "{stroke:?} {transform:?}");
                    }
                }
            }
        }
    }
    assert_eq!(compared, 5 * 4 * 5 * 3);
}

/// Paths for [`adjusted_pieces_cover_the_fitted_components`], and whether
/// each is axis-aligned.
fn adjusted_paths() -> Vec<(Path, bool)> {
    let mut paths = Vec::new();
    let mut rectangle = PathBuilder::new();
    rectangle.rect(4.3, 5.6, 21.0, 13.2);
    paths.push((rectangle.finish(), true));
    let mut corner = PathBuilder::new();
    corner.polyline([(3.2, 30.4), (3.2, 6.1), (27.7, 6.1)].map(|(x, y)| Point::new(x, y)));
    paths.push((corner.finish(), true));
    for closed in [false, true] {
        let mut zigzag = PathBuilder::new();
        zigzag.polyline(
            [
                (2.0, 2.5),
                (14.0, 4.0),
                (5.0, 12.0),
                (21.0, 20.5),
                (6.0, 31.0),
            ]
            .map(|(x, y)| Point::new(x, y)),
        );
        if closed {
            zigzag.close();
        }
        paths.push((zigzag.finish(), false));
    }
    let mut dot = PathBuilder::new();
    dot.polyline([Point::new(17.3, 17.8), Point::new(17.3, 17.8)]);
    paths.push((dot.finish(), false));
    paths
}

#[test]
fn assembled_outlines_keep_only_their_output_charged() {
    // A closed star's spikes overlap, so its pieces are unproven: taken as
    // they are, or replaced by the components where only proven pieces are.
    let points: Vec<Point> = (0..80_u32)
        .map(|i| {
            let angle = std::f64::consts::TAU * f64::from(i) / 80.0;
            let radius = if i % 2 == 0 { 200.0 } else { 20.0 };
            Point::new(radius * angle.cos(), radius * angle.sin())
        })
        .collect();
    let stroke = Stroke {
        width: 30.0,
        join: LineJoin::Round,
        ..Stroke::default()
    };
    let pen = Pen {
        radius: 15.0,
        tolerance: TOLERANCE,
    };
    for (accept, taken) in [
        (Tiling::Unproven, Tiling::Unproven),
        (Tiling::Proven, Tiling::Components),
    ] {
        let runs = vec![Run {
            points: points.clone(),
            closed: true,
            tangent: Point::new(1.0, 0.0),
        }];
        let mut bytes = crate::testing::bytes();
        let (out, _, tiling) = assemble(
            runs,
            (pen, Transform::IDENTITY, false),
            &stroke,
            &Limits::default(),
            accept,
            |_| PathBuilder::new(),
            &mut bytes,
        )
        .unwrap();
        assert_eq!(tiling, taken);
        assert_eq!(bytes.used(), out.heap_bytes(), "{accept:?}");
    }
}

#[test]
fn round_joins_within_one_step_take_their_normals_only() {
    // The arc's own count of steps between the normals, as `join_into`
    // finds it where the turn exceeds a step.
    let steps = |a: Point, b: Point, pen: Pen| {
        let cross = a.cross(b);
        let side = if cross > 0.0 { -pen.radius } else { pen.radius };
        let (n1, n2) = (a.perp() * side, b.perp() * side);
        let (start, end) = (n1.y.atan2(n1.x), n2.y.atan2(n2.x));
        let angle = if cross > 0.0 {
            (end - start).rem_euclid(std::f64::consts::TAU)
        } else {
            -(start - end).rem_euclid(std::f64::consts::TAU)
        };
        let step = 2.0 * (1.0 - (pen.tolerance / pen.radius).min(1.0)).acos();
        (angle.abs() / step).ceil().max(1.0)
    };
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        #[expect(
            clippy::cast_precision_loss,
            reason = "53 random bits map to a uniform fraction"
        )]
        let unit = (state >> 11) as f64 / (1u64 << 53) as f64;
        unit
    };
    let stroke = Stroke {
        join: LineJoin::Round,
        ..Stroke::default()
    };
    let mut counts = [0_usize; 2];
    for case in 0..200_000 {
        let radius = 0.01 + 20.0 * next() * next();
        let pen = Pen {
            radius,
            tolerance: TOLERANCE / (0.2 + 8.0 * next()),
        };
        let heading = next() * std::f64::consts::TAU;
        // Turns around one step, and anywhere.
        let step = 2.0 * (1.0 - (pen.tolerance / radius).min(1.0)).acos();
        let turn = if case % 2 == 0 {
            step * (0.999_999 + 2e-6 * next()) * if next() < 0.5 { 1.0 } else { -1.0 }
        } else {
            (next() - 0.5) * std::f64::consts::TAU
        };
        let a = Point::new(heading.cos(), heading.sin());
        let b = Point::new((heading + turn).cos(), (heading + turn).sin());
        let mut points = Vec::new();
        let drawn = join_into(
            Point::new(3.0, 4.0),
            [a, b],
            pen,
            &stroke,
            &Limits::default(),
            &mut points,
            &mut crate::testing::bytes(),
        )
        .unwrap();
        if drawn.is_none() {
            continue;
        }
        let count = steps(a, b, pen);
        assert_eq!(
            f64::from(u32::try_from(points.len()).unwrap()),
            count + 1.0,
            "{a:?} {b:?} {radius} {}",
            pen.tolerance
        );
        if case % 2 == 0 {
            counts[usize::from(count > 1.0)] += 1;
        }
    }
    // Turns around one step fall on both sides of it.
    assert!(counts[0] > 1000 && counts[1] > 1000, "{counts:?}");
}
