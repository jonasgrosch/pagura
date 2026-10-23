use super::*;

#[test]
fn bounded_clone_preserves_certificates_and_point_bits() {
    let mut builder = PathBuilder::new();
    builder.move_to(Point::new(-0.0, 0.0)).close();
    let mut path = builder.finish();
    path.fill_certificates = [Some(FillRegion::Points), Some(FillRegion::EmptyBoundary)];
    let needed = size_of_val(path.segments());
    let mut refused = Meter::new(needed - 1, "outline bytes");
    assert_eq!(
        path.clone_with_bytes(&mut refused).unwrap_err().message,
        "outline bytes"
    );
    assert_eq!(refused.used(), 0);
    let mut exact = Meter::new(needed, "outline bytes");
    let copy = path.clone_with_bytes(&mut exact).unwrap();
    assert_eq!(copy, path);
    let Segment::MoveTo(point) = copy.segments()[0] else {
        panic!("the first point changed");
    };
    assert_eq!(point.x.to_bits(), (-0.0_f64).to_bits());
    assert_eq!(point.y.to_bits(), 0.0_f64.to_bits());
    assert_eq!(exact.used(), copy.retained_bytes());
}

#[test]
fn rect_paths_match_re_and_keep_exact_corners() {
    let rect = Rect::from_corners(7.0, 0.1, -3.0, 0.3).unwrap();
    let mut re = PathBuilder::new();
    re.rect(-3.0, 0.1, 10.0, 0.2);
    let expected = re.finish();
    let path = Path::rect(rect);
    assert_eq!(path.segments()[..2], expected.segments()[..2]);
    // 0.1 + 0.2 != 0.3 in f64: the corners come from the rectangle itself.
    assert_eq!(
        path.segments()[2..],
        [
            Segment::LineTo(Point::new(7.0, 0.3)),
            Segment::LineTo(Point::new(-3.0, 0.3)),
            Segment::Close
        ]
    );
}

#[test]
fn metered_rectangles_preserve_segments_and_refuse_before_allocation() {
    for rect in [
        Rect::from_corners(7.0, 0.1, -3.0, 0.3).unwrap(),
        Rect::from_corners(-0.0, -0.0, 0.0, 0.0).unwrap(),
        Rect::from_corners(-1e100, -2e100, 3e100, 4e100).unwrap(),
    ] {
        let required = 5 * size_of::<Segment>();
        let mut refused = Meter::new(required - 1, "rectangle memory");
        let error = Path::rect_with_bytes(rect, &mut refused).unwrap_err();
        assert_eq!(error.message, "rectangle memory");
        assert_eq!(refused.used(), 0);
        let mut exact = Meter::new(11 + required, "rectangle memory");
        exact.charge(11).unwrap();
        let path = Path::rect_with_bytes(rect, &mut exact).unwrap();
        assert_eq!(path, Path::rect(rect));
        assert_eq!(path.segments().len(), 5);
        assert_eq!(exact.used(), 11 + path.retained_bytes());
        for (segment, corner) in path.segments().iter().zip(Path::rect_corners(rect)) {
            let (Segment::MoveTo(point) | Segment::LineTo(point)) = segment else {
                panic!("rectangle corner was not retained");
            };
            assert_eq!(
                [point.x.to_bits(), point.y.to_bits()],
                [corner.x.to_bits(), corner.y.to_bits()]
            );
        }
    }
}

#[test]
fn metered_classification_keeps_work_cache_and_live_path_bytes() {
    let mut builder = PathBuilder::new();
    builder.polygon([
        Point::new(0.0, 0.0),
        Point::new(2.0, 0.0),
        Point::new(0.0, 0.0),
    ]);
    let source = builder.finish();
    let limits = Limits::default();
    for rule in [FillRule::NonZero, FillRule::EvenOdd] {
        let mut path = source.clone();
        let retained = path.retained_bytes();
        let mut tight = Meter::new(retained, "classification memory");
        tight.charge(retained).unwrap();
        let mut work = 0;
        let error = path
            .classify_fill_with_bytes(rule, &limits, &mut work, &mut tight)
            .unwrap_err();
        assert_eq!(error.message, "classification memory");
        assert_eq!(tight.used(), retained);
        assert!(path.fill_region(rule).is_none());
        let mut bytes = Meter::new(1 << 20, "classification memory");
        bytes.charge(retained).unwrap();
        work = 0;
        path.classify_fill_with_bytes(rule, &limits, &mut work, &mut bytes)
            .unwrap();
        assert_eq!(path.fill_region(rule), Some(FillRegion::EmptyBoundary));
        assert_eq!(bytes.used(), retained, "temporary storage was released");
        let mut compatibility = source.clone();
        let mut expected_work = 0;
        compatibility
            .classify_fill(rule, &limits, &mut expected_work)
            .unwrap();
        assert_eq!(path, compatibility);
        assert_eq!(work, expected_work);
        let mut empty = Meter::new(0, "cached classification");
        path.classify_fill_with_bytes(rule, &limits, &mut work, &mut empty)
            .unwrap();
        assert_eq!(work, expected_work);
        assert_eq!(empty.used(), 0);
    }
}

#[test]
fn lines_and_curves_need_a_current_point() {
    let mut path = PathBuilder::new();
    assert_eq!(
        path.line_to(Point::new(1.0, 0.0)).err(),
        Some(NoCurrentPoint)
    );
    assert_eq!(
        path.cubic_to(Point::default(), Point::default(), Point::default())
            .err(),
        Some(NoCurrentPoint)
    );
    assert!(path.close().is_empty());
    path.move_to(Point::new(1.0, 1.0)).close();
    // Close returns the current point to the subpath start (8.5.2.1).
    path.line_to(Point::new(2.0, 1.0)).unwrap();
    assert_eq!(path.current_point(), Some(Point::new(2.0, 1.0)));
}

#[test]
fn polylines_polygons_and_appended_paths_keep_their_order() {
    let mut path = PathBuilder::new();
    path.polygon([]);
    assert!(path.is_empty());
    let [a, b, c] = [
        Point::new(0.0, 0.0),
        Point::new(2.0, 0.0),
        Point::new(0.0, 2.0),
    ];
    path.polyline([a, b, c]);
    assert_eq!(path.current_point(), Some(c));
    let open = path.finish();
    path.polygon([a, b, c]);
    assert_eq!(path.current_point(), Some(a));
    let triangle = path.finish();
    assert_eq!(
        triangle.segments(),
        [
            Segment::MoveTo(a),
            Segment::LineTo(b),
            Segment::LineTo(c),
            Segment::Close
        ]
    );
    assert_eq!(open.segments(), &triangle.segments()[..3]);
    let mut curve = PathBuilder::new();
    curve.move_to(a).cubic_to(b, c, b).unwrap();
    let curve = curve.finish();
    let shift = Transform::translate(1.0, -1.0);
    let mut combined = PathBuilder::new();
    combined
        .move_to(c)
        .append(&triangle, shift)
        .append(&curve, shift);
    assert_eq!(combined.current_point(), Some(b * shift));
    let expected: Vec<_> = [Segment::MoveTo(c)]
        .into_iter()
        .chain(triangle.transformed(shift).segments().iter().copied())
        .chain(curve.transformed(shift).segments().iter().copied())
        .collect();
    assert_eq!(combined.finish().segments(), expected);
}
