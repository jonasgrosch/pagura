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
fn parameters_bracket_heights_just_outside() {
    // A piece leaving its top level, where the root is ill conditioned, a
    // steep one, and one running down to the left.
    for points in [
        [(0.0, 0.0), (3.0, 0.0), (4.0, 5.0)],
        [(1.0, 0.0), (1.5, 6.0), (4.0, 7.0)],
        [(9.0, -2.0), (2.0, 1.0), (0.5, 3.0)],
    ] {
        let m = Mono::new(Quadratic {
            points: points.map(|(x, y)| Point::new(x, y)),
        });
        let height = m.y1 - m.y0;
        for (lo, hi) in [
            (0.0, 1.0),
            (1e-9, 0.25),
            (0.3, 0.3 + 1e-7),
            (0.5, 1.0),
            (0.999_999, 1.0),
        ] {
            let (lo, hi) = (lo * height, hi * height);
            let [first, last] = m.parameters(m.y0 + lo, m.y0 + hi);
            let (t, u) = (m.ty(lo), m.ty(hi));
            assert!(0.0 <= first && first <= t && t <= u && u <= last && last <= 1.0);
            assert!(first == 0.0 || m.dy(first) < lo);
            assert!(last == 1.0 || m.dy(last) > hi);
            assert!(t - first <= 1.0 / 1024.0 && last - u <= 1.0 / 1024.0);
        }
    }
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
