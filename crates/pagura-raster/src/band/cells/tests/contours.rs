use super::super::*;

fn bits(values: &[f64]) -> Vec<u64> {
    values.iter().copied().map(f64::to_bits).collect()
}

fn compare<const MEASURE: bool>(
    points: &[Point],
    origin: Point,
    columns: (usize, usize),
    rows: std::ops::Range<u32>,
    skip: u32,
) {
    let new = || Cells::new(64, &mut Meter::new(usize::MAX, "bytes")).unwrap();
    let (mut expected, mut actual) = (new(), new());
    assert!(expected.open(columns, rows.clone()));
    assert!(actual.open(columns, rows.clone()));
    expected.skip(skip);
    actual.skip(skip);
    let retained = actual.retained_bytes();
    let at = |point: Point| (point + origin) + Point::default();
    let mut work = 0;
    // The existing individual-line entry is the reference, not a second
    // implementation of the whole-row admission or cell arithmetic.
    for (&a, &b) in points.iter().zip(points.iter().cycle().skip(1)) {
        let line = Line { a: at(a), b: at(b) };
        if !line.is_horizontal() && expected.reaches(line.a.x.min(line.b.x)) {
            work += if MEASURE {
                expected.line_measured(line)
            } else {
                expected.line(line)
            };
        }
    }
    assert_eq!(actual.contour::<MEASURE>(points, origin), work);
    assert_eq!(actual.retained_bytes(), retained);
    assert_eq!(bits(&actual.area), bits(&expected.area));
    assert_eq!(bits(&actual.cover), bits(&expected.cover));
    assert_eq!(actual.touched, expected.touched);
    assert_eq!(
        actual.rounding.absolute.to_bits(),
        expected.rounding.absolute.to_bits()
    );
    assert_eq!(
        actual.rounding.primitive.to_bits(),
        expected.rounding.primitive.to_bits()
    );
    assert_eq!(actual.rounding.additions, expected.rounding.additions);
    assert_eq!(
        actual.arithmetic_error().to_bits(),
        expected.arithmetic_error().to_bits()
    );
    for (r, _) in (0_usize..).zip(rows) {
        let (mut a, mut b) = (Vec::new(), Vec::new());
        actual.inspect(r, |x0, x1, value| a.push((x0, x1, value.to_bits())));
        expected.inspect(r, |x0, x1, value| b.push((x0, x1, value.to_bits())));
        assert_eq!(a, b);
    }
}

#[test]
fn bulk_contours_keep_raw_cells_work_and_measurement_across_windows() {
    let tiny = f64::from_bits(1);
    let cases = [
        vec![],
        vec![Point::new(-0.0, tiny)],
        vec![
            Point::new(1.125, 2.125),
            Point::new(1.875, 2.875),
            Point::new(2.25, 2.25),
            Point::new(2.5, 3.0),
            Point::new(1.125, 2.125),
        ],
        vec![
            Point::new(-4.0, 1.5),
            Point::new(0.25, 1.75),
            Point::new(4.5, 1.125),
            Point::new(18.0, 2.5),
            Point::new(1.0, 6.25),
        ],
        vec![
            Point::new(-0.0, -0.0),
            Point::new(tiny, tiny),
            Point::new(1.0, 0.5),
            Point::new(1.0, 1.0),
            Point::new(0.0, 0.5),
        ],
        vec![
            Point::new(-549_755_813_888.0, -1.0),
            Point::new(549_755_813_888.0, 4.0),
            Point::new(0.75, 4.0),
        ],
    ];
    for points in &cases {
        for origin in [
            Point::new(-0.0, -0.0),
            Point::new(0.25, 0.125),
            Point::new(-0.75, 7.75),
        ] {
            for (columns, rows, skip) in [
                ((0, 16), 0..16, 0),
                ((1, 8), 0..3, 0b010),
                ((0, 8), 1..9, 0b101),
                ((0, 16), 0..32, 1 << 31),
                ((2, 12), 7..16, 0),
            ] {
                compare::<false>(points, origin, columns, rows.clone(), skip);
                compare::<true>(points, origin, columns, rows, skip);
            }
        }
    }
}

#[test]
fn bulk_contours_keep_same_row_cancellation_and_closing_order() {
    // Alternating orientations revisit identical cells many times, with
    // a crossing and a horizontal side interrupting the short-edge runs.
    let mut points = Vec::new();
    for index in 0..128_u32 {
        points.push(Point::new(2.125, 3.125));
        points.push(Point::new(2.875, 3.875));
        if index % 2 == 0 {
            points.push(Point::new(2.125, 3.875));
            points.push(Point::new(2.875, 3.125));
        }
    }
    points.extend([Point::new(-0.5, 1.0), Point::new(5.0, 1.0)]);
    compare::<false>(&points, Point::default(), (1, 7), 0..8, 0);
    compare::<true>(&points, Point::default(), (1, 7), 0..8, 0);
}
