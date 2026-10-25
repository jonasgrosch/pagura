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
        scratch: None,
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
