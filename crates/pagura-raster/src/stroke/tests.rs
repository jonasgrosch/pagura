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
