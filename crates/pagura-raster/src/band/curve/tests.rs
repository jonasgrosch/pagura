use super::*;
use crate::PathBuilder;

/// The integral of x dy along the pieces: the signed area they bound.
fn area(spline: &Spline) -> f64 {
    spline
        .pieces
        .iter()
        .map(|p| match *p {
            Piece::Line(Line { a, b }) => (b.y - a.y) * (a.x + b.x) * 0.5,
            Piece::Curve(q) => {
                let m = Mono::new(q);
                m.direction * ((m.y1 - m.y0) * m.x0 + m.h(1.0))
            }
        })
        .sum()
}

#[test]
fn elevated_quadratics_split_into_monotone_pieces() {
    // The quadratic (0,0) (2,4) (4,0) as a cubic.
    let mut path = PathBuilder::new();
    path.move_to(Point::new(0.0, 0.0));
    let c = |a: f64, b: f64| a + (b - a) * 2.0 / 3.0;
    path.cubic_to(
        Point::new(c(0.0, 2.0), c(0.0, 4.0)),
        Point::new(c(4.0, 2.0), c(0.0, 4.0)),
        Point::new(4.0, 0.0),
    )
    .unwrap();
    path.close();
    let spline = spline(
        &path.finish(),
        Transform::IDENTITY,
        &Limits::default(),
        &mut crate::testing::bytes(),
    )
    .unwrap()
    .unwrap();
    let curves = spline
        .pieces
        .iter()
        .filter(|p| matches!(p, Piece::Curve(_)))
        .count();
    assert_eq!(curves, 2);
    // The area under the arch, 2/3 × 4 × 2 = 16/3.
    assert!((area(&spline).abs() - 16.0 / 3.0).abs() < 1e-12);
}

#[test]
fn a_cancelled_third_difference_does_not_hide_mapping_roundoff() {
    let mut path = PathBuilder::new();
    path.move_to(Point::new(1e12, 1e12));
    path.cubic_to(
        Point::new(1e12 + 2.0, 1e12 + 4.0),
        Point::new(1e12 + 4.0, 1e12 + 4.0),
        Point::new(1e12 + 6.0, 1e12),
    )
    .unwrap();
    path.close();
    let path = path.finish();
    assert!(!reducible_with_rounding(&path, Transform::IDENTITY));
    assert!(
        spline(
            &path,
            Transform::IDENTITY,
            &Limits::default(),
            &mut crate::testing::bytes()
        )
        .unwrap()
        .is_none()
    );
}

#[test]
fn general_cubics_keep_the_canonical_polygon() {
    let mut path = PathBuilder::new();
    path.move_to(Point::new(100.0, 0.0));
    path.cubic_to(
        Point::new(100.0, 55.0),
        Point::new(55.0, 100.0),
        Point::new(0.0, 100.0),
    )
    .unwrap();
    path.line_to(Point::new(0.0, 0.0)).unwrap();
    path.close();
    assert!(
        spline(
            &path.finish(),
            Transform::IDENTITY,
            &Limits::default(),
            &mut crate::testing::bytes()
        )
        .unwrap()
        .is_none()
    );
}

#[test]
fn points_are_left_to_the_polygon() {
    let mut path = PathBuilder::new();
    path.move_to(Point::new(1.0, 1.0));
    path.close();
    assert!(
        spline(
            &path.finish(),
            Transform::IDENTITY,
            &Limits::default(),
            &mut crate::testing::bytes()
        )
        .unwrap()
        .is_none()
    );
}

#[test]
fn spline_builder_checks_each_allocation_and_refunds_failed_storage() {
    let mut path = PathBuilder::new();
    path.rect(0.0, 0.0, 4.0, 4.0);
    let path = path.finish();
    let source = path.retained_bytes();
    let mut bytes = Meter::new(
        source + size_of::<usize>() + size_of::<Piece>() - 1,
        "spline bytes",
    );
    bytes.charge(source).unwrap();
    let error = spline(&path, Transform::IDENTITY, &Limits::default(), &mut bytes).unwrap_err();
    assert_eq!(
        (error.kind, error.message),
        (pagura_core::ErrorKind::Limit, "spline bytes")
    );
    assert_eq!(bytes.used(), source);
    assert_eq!(path.segments().len(), 5);
}

fn quadratic_body(path: &mut PathBuilder) {
    path.move_to(Point::new(0.0, 0.0));
    path.cubic_to(
        Point::new(2.0, 2.0),
        Point::new(4.0, 2.0),
        Point::new(6.0, 0.0),
    )
    .unwrap();
    path.close();
}

fn piece_bits(piece: Piece) -> (bool, [[u64; 2]; 3]) {
    let points = match piece {
        Piece::Line(Line { a, b }) => [a, b, b],
        Piece::Curve(q) => q.points,
    };
    (
        matches!(piece, Piece::Curve(_)),
        points.map(|p| [p.x.to_bits(), p.y.to_bits()]),
    )
}

#[test]
fn glyph_spline_collects_points_in_canonical_contour_order() {
    let mut body = PathBuilder::new();
    quadratic_body(&mut body);
    let body = spline(
        &body.finish(),
        Transform::IDENTITY,
        &Limits::default(),
        &mut crate::testing::bytes(),
    )
    .unwrap()
    .unwrap();
    let [explicit, line, cubic, earlier, next] =
        std::array::from_fn(|i| Point::new(10.0 + f64::from(u32::try_from(i).unwrap()), 1.0));
    let mut path = PathBuilder::new();
    quadratic_body(&mut path);
    path.move_to(explicit);
    path.close();
    path.close();
    path.move_to(line);
    path.line_to(line).unwrap();
    path.move_to(cubic);
    path.cubic_to(cubic, cubic, cubic).unwrap();
    path.close();
    path.move_to(earlier);
    path.move_to(next);
    path.close();
    path.move_to(Point::new(99.0, 99.0));
    let result = glyph_spline(
        &path.finish(),
        Transform::IDENTITY,
        &Limits::default(),
        &mut crate::testing::bytes(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(result.points, [explicit, line, cubic, earlier, next]);
    assert_eq!(
        result
            .pieces
            .into_iter()
            .map(piece_bits)
            .collect::<Vec<_>>(),
        body.pieces.into_iter().map(piece_bits).collect::<Vec<_>>()
    );
}

#[test]
fn glyph_spline_collects_close_continuations_at_the_subpath_start() {
    let mut path = PathBuilder::new();
    quadratic_body(&mut path);
    let point = Point::new(10.0, 1.0);
    path.move_to(point);
    path.line_to(Point::new(11.0, 2.0)).unwrap();
    path.close();
    path.line_to(point).unwrap();
    path.close();
    path.cubic_to(point, point, point).unwrap();
    path.close();
    let result = glyph_spline(
        &path.finish(),
        Transform::IDENTITY,
        &Limits::default(),
        &mut crate::testing::bytes(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(result.points, [point, point]);
}

#[test]
fn glyph_spline_classifies_mapped_original_controls() {
    let transform = Transform::new(1.0, 1.0, 1.0, 2.0, 0.0, 0.0);
    let point = Point::new(100.0, 1.0);
    let other = Point::new(100.0, 1.0_f64.next_up());
    assert_ne!(point, other);
    assert_ne!(transform.determinant(), 0.0);
    assert_eq!(point * transform, other * transform);
    let mut path = PathBuilder::new();
    quadratic_body(&mut path);
    path.move_to(point);
    path.line_to(other).unwrap();
    path.close();
    path.move_to(point);
    path.cubic_to(other, other, other).unwrap();
    path.close();
    let path = path.finish();
    let result = glyph_spline(
        &path,
        transform,
        &Limits::default(),
        &mut crate::testing::bytes(),
    )
    .unwrap()
    .unwrap();
    let mapped = point * transform;
    assert_eq!(
        result
            .points
            .iter()
            .map(|p| [p.x.to_bits(), p.y.to_bits()])
            .collect::<Vec<_>>(),
        [[mapped.x.to_bits(), mapped.y.to_bits()]; 2]
    );
    // The ordinary entry point retains its existing source-only policy.
    let ordinary = spline(
        &path,
        transform,
        &Limits::default(),
        &mut crate::testing::bytes(),
    )
    .unwrap()
    .unwrap();
    assert!(ordinary.points.is_empty());
}

#[test]
fn glyph_spline_leaves_a_points_only_source_to_the_polygon() {
    let mut path = PathBuilder::new();
    path.move_to(Point::new(1.0, 1.0));
    path.close();
    path.move_to(Point::new(2.0, 2.0));
    path.line_to(Point::new(2.0, 2.0)).unwrap();
    let mut bytes = crate::testing::bytes();
    bytes.charge(37).unwrap();
    assert!(
        glyph_spline(
            &path.finish(),
            Transform::IDENTITY,
            &Limits::default(),
            &mut bytes
        )
        .unwrap()
        .is_none()
    );
    assert_eq!(bytes.used(), 37);
}

#[test]
fn glyph_spline_charges_four_segments_for_each_point() {
    let mut path = PathBuilder::new();
    quadratic_body(&mut path);
    for _ in 0..4 {
        path.move_to(Point::new(10.0, 1.0));
        path.close();
    }
    let path = path.finish();
    let mut bytes = crate::testing::bytes();
    bytes.charge(37).unwrap();
    let limits = Limits {
        path_segments: 18,
        ..Limits::default()
    };
    let error = glyph_spline(&path, Transform::IDENTITY, &limits, &mut bytes).unwrap_err();
    assert_eq!(error.message, "quadratic edge budget exceeded");
    assert_eq!(bytes.used(), 37);
    let limits = Limits {
        path_segments: 19,
        ..Limits::default()
    };
    let result = glyph_spline(&path, Transform::IDENTITY, &limits, &mut bytes)
        .unwrap()
        .unwrap();
    assert_eq!(result.pieces.len() + 4 * result.points.len(), 19);
    assert_eq!(bytes.used(), 37 + result.retained_bytes());
}

#[test]
fn glyph_spline_accounts_point_capacity_and_refunds_failure() {
    let mut path = PathBuilder::new();
    quadratic_body(&mut path);
    for _ in 0..8 {
        path.move_to(Point::new(10.0, 1.0));
        path.close();
    }
    let path = path.finish();
    let mut bytes = crate::testing::bytes();
    let result = glyph_spline(&path, Transform::IDENTITY, &Limits::default(), &mut bytes)
        .unwrap()
        .unwrap();
    assert_eq!(bytes.used(), result.retained_bytes());
    assert_eq!(result.points.len(), 8);
    assert!(result.retained_bytes() >= result.points.capacity() * size_of::<Point>());
    let mut tight = Meter::new(37 + result.retained_bytes() - 1, "glyph point bytes");
    tight.charge(37).unwrap();
    let error =
        glyph_spline(&path, Transform::IDENTITY, &Limits::default(), &mut tight).unwrap_err();
    assert_eq!(error.message, "glyph point bytes");
    assert_eq!(tight.used(), 37);
}
