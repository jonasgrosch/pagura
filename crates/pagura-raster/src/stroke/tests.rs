use super::*;

#[test]
fn source_curve_miter_has_the_analytic_triangle_area() {
    // At (16,23.5), the source directions are (-1,-2) and (-1,2).
    // The exterior miter has apex y=23.5-sqrt(5)/2 and side slope1/2.
    // Only that triangle meets x[15.5,16], y[22.5,23]: its device area
    // after2x scaling is sqrt(5)/2-3/4, which quantizes to94/255.
    for closed in [false, true] {
        let mut path = PathBuilder::new();
        if closed {
            path.move_to(Point::new(16.0, 23.5));
            path.line_to(Point::new(8.5, 38.5)).unwrap();
            path.line_to(Point::new(23.5, 23.5)).unwrap();
        } else {
            path.move_to(Point::new(23.5, 23.5));
        }
        path.quad_to(Point::new(19.75, 31.0), Point::new(16.0, 23.5))
            .unwrap();
        if closed {
            path.close();
        } else {
            path.line_to(Point::new(8.5, 38.5)).unwrap();
        }
        let path = path.finish();
        let transform = Transform::new(2.0, 0.0, 0.0, 2.0, -31.0, -45.0);
        for dash_array in [vec![], vec![1000.0, 1.0]] {
            let stroke = Stroke {
                dash_array,
                ..Stroke::default()
            };
            let actual = crate::testing::stroke(
                crate::testing::window(1, 1),
                &path,
                &stroke,
                transform,
                &Limits::default(),
            )
            .unwrap();
            assert_eq!(actual, [94], "closed={closed}");
            let (outlined, pen) =
                stroke_outline(&path, transform, &stroke, &Limits::default()).unwrap();
            let filled = crate::testing::fill(
                crate::testing::window(1, 1),
                (&outlined, pen, crate::FillRule::NonZero),
                &[],
                &Limits::default(),
            )
            .unwrap();
            assert_eq!(filled, [94]);
        }
    }
}

#[test]
fn smooth_curve_joins_retain_equal_source_directions_through_zero_segments() {
    for curved in [false, true] {
        let mut path = PathBuilder::new();
        path.move_to(Point::new(0.0, 0.0));
        path.quad_to(Point::new(0.0, 8.0), Point::new(8.0, 8.0))
            .unwrap();
        for _ in 0..32 {
            path.line_to(Point::new(8.0, 8.0)).unwrap();
        }
        if curved {
            path.quad_to(Point::new(16.0, 8.0), Point::new(16.0, 16.0))
                .unwrap();
        } else {
            path.line_to(Point::new(16.0, 8.0)).unwrap();
        }
        let mut bytes = crate::testing::bytes();
        let mut contours = flatten(
            &path.finish(),
            Transform::IDENTITY,
            Source::Stroke,
            &Limits::default(),
            &mut bytes,
        )
        .unwrap();
        let runs = dash_contour(
            contours.remove(0),
            Transform::IDENTITY,
            &Stroke::default(),
            &Limits::default(),
            &mut bytes,
        )
        .unwrap();
        let run = &runs[0];
        let join = run
            .joins
            .iter()
            .find(|join| run.points[join.index] == Point::new(8.0, 8.0))
            .unwrap();
        assert_eq!(join.tangents, [Point::new(1.0, 0.0); 2]);
    }
}

#[test]
fn source_vertices_supply_dash_cap_directions() {
    let limits = Limits::default();
    for closed in [false, true] {
        let mut path = PathBuilder::new();
        path.move_to(Point::new(4.0, 4.0));
        path.cubic_to(
            Point::new(4.0, 12.0),
            Point::new(12.0, 12.0),
            Point::new(12.0, 4.0),
        )
        .unwrap();
        if closed {
            path.close();
        }
        let path = path.finish();
        for (dash_array, dash_phase) in [
            (vec![], 0.0),
            (vec![100.0, 1.0], 0.0),
            (vec![1.0, 1.0], 1.0),
        ] {
            let stroke = Stroke {
                dash_array,
                dash_phase,
                ..Stroke::default()
            };
            let mut bytes = crate::testing::bytes();
            let mut contours = flatten(
                &path,
                Transform::IDENTITY,
                Source::Dashed,
                &limits,
                &mut bytes,
            )
            .unwrap();
            let runs = dash_contour(
                contours.remove(0),
                Transform::IDENTITY,
                &stroke,
                &limits,
                &mut bytes,
            )
            .unwrap();
            assert!(!runs.is_empty(), "every pattern paints part of the arch");
            if closed {
                for run in &runs {
                    if run.closed {
                        assert_eq!(run.caps, [None; 2]);
                        continue;
                    }
                    let tangent = |point: Option<&Point>, start: bool| match point.copied() {
                        Some(Point { x: 4.0, y: 4.0 }) => Some(if start {
                            Point::new(0.0, 1.0)
                        } else {
                            Point::new(-1.0, 0.0)
                        }),
                        Some(Point { x: 12.0, y: 4.0 }) => Some(if start {
                            Point::new(-1.0, 0.0)
                        } else {
                            Point::new(0.0, -1.0)
                        }),
                        _ => None,
                    };
                    for (actual, expected) in run.caps.into_iter().zip([
                        tangent(run.points.first(), true),
                        tangent(run.points.last(), false),
                    ]) {
                        if expected.is_some() {
                            assert_eq!(actual, expected);
                        }
                    }
                }
            } else if dash_phase == 0.0 {
                assert_eq!(
                    runs[0].caps,
                    [Some(Point::new(0.0, 1.0)), Some(Point::new(0.0, -1.0))]
                );
            } else {
                assert!(runs.iter().all(|run| run.caps[0].is_some()));
            }
        }
    }
}

#[test]
fn curved_dash_cuts_follow_user_arclength_and_true_tangents() {
    // q(t)=(8t,8t²); integrating |q'(t)| gives this independent
    // antiderivative. The cuts at t=1/3 and t=2/3 lie inside dyadic subdivision pieces.
    let length = |t: f64| 4.0 * t * (1.0 + 4.0 * t * t).sqrt() + 2.0 * (2.0 * t).asinh();
    let [first, second] = [1.0 / 3.0, 2.0 / 3.0];
    let stroke = Stroke {
        dash_array: vec![length(first), length(second) - length(first), 1000.0],
        ..Stroke::default()
    };
    let mut path = PathBuilder::new();
    path.move_to(Point::default());
    path.quad_to(Point::new(4.0, 0.0), Point::new(8.0, 8.0))
        .unwrap();
    let path = path.finish();
    let limits = Limits::default();
    for transform in [Transform::IDENTITY, Transform::scale(11.0, 3.0)] {
        let mut bytes = crate::testing::bytes();
        let mut contours = flatten(&path, transform, Source::Dashed, &limits, &mut bytes).unwrap();
        let runs =
            dash_contour(contours.remove(0), transform, &stroke, &limits, &mut bytes).unwrap();
        assert_eq!(runs.len(), 2);
        for (run, end, t) in [(&runs[0], 1, first), (&runs[1], 0, second)] {
            let point = if end == 0 {
                run.points.first()
            } else {
                run.points.last()
            }
            .unwrap();
            assert!((*point - Point::new(8.0 * t, 8.0 * t * t)).length() < TOLERANCE / 64.0);
            let direction = Point::new(1.0, 2.0 * t);
            assert!((run.caps[end].unwrap() - direction / direction.length()).length() < 1e-5);
        }
    }
    let mut bytes = crate::testing::bytes();
    let mut contours = flatten(
        &path,
        Transform::IDENTITY,
        Source::Dashed,
        &limits,
        &mut bytes,
    )
    .unwrap();
    let limits = Limits {
        raster_work: 0,
        ..limits
    };
    assert_eq!(
        dash_contour(
            contours.remove(0),
            Transform::IDENTITY,
            &stroke,
            &limits,
            &mut bytes
        )
        .err()
        .unwrap()
        .message,
        RASTER_WORK,
    );
}

#[test]
fn curved_dash_vertices_stay_behind_a_nearby_butt_cap() {
    // q(t)=(8t,8t²), ending just after the first dyadic subdivision vertex
    // at this 2x transform. The pixel x[0,1/2],y[1/4,3/4] is wholly between
    // the two offsets; only the start and end cap planes intersect it.
    // Integrating x_end(y)=8t-2t(y-8t²), then scaling the area by 4, gives
    // 14t+32t³. For t=65/4096 this quantizes independently to 57/255.
    let t: f64 = 65.0 / 4096.0;
    let length = 4.0 * t * (1.0 + 4.0 * t * t).sqrt() + 2.0 * (2.0 * t).asinh();
    let stroke = Stroke {
        width: 2.0,
        dash_array: vec![length, 1000.0],
        ..Stroke::default()
    };
    let mut path = PathBuilder::new();
    path.move_to(Point::default());
    path.quad_to(Point::new(4.0, 0.0), Point::new(8.0, 8.0))
        .unwrap();
    let path = path.finish();
    let transform = Transform::new(2.0, 0.0, 0.0, 2.0, 0.0, -0.5);
    let limits = Limits::default();
    assert_eq!(
        crate::testing::stroke(
            crate::testing::window(1, 1),
            &path,
            &stroke,
            transform,
            &limits
        )
        .unwrap(),
        [57]
    );
    let (outline, pen) = stroke_outline(&path, transform, &stroke, &limits).unwrap();
    assert_eq!(
        crate::testing::fill(
            crate::testing::window(1, 1),
            (&outline, pen, crate::FillRule::NonZero),
            &[],
            &limits,
        )
        .unwrap(),
        [57]
    );
}

#[test]
fn only_dashed_curves_retain_subdivision_controls() {
    let mut path = PathBuilder::new();
    path.move_to(Point::default());
    path.quad_to(Point::new(0.0, 8.0), Point::new(8.0, 8.0))
        .unwrap();
    let path = path.finish();
    for source in [Source::Fill, Source::Stroke, Source::Dashed] {
        let contours = flatten(
            &path,
            Transform::IDENTITY,
            source,
            &Limits::default(),
            &mut crate::testing::bytes(),
        )
        .unwrap();
        assert_eq!(contours[0].pieces.is_empty(), source != Source::Dashed);
    }
}

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
    let error = dash(
        points,
        Vec::new(),
        (Vec::new(), TOLERANCE),
        false,
        &stroke,
        &Limits::default(),
        &mut bytes,
    )
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
            Vec::new(),
            (Vec::new(), TOLERANCE),
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
