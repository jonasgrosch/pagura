use super::*;
use crate::{GlyphGeometry, PathBuilder};

#[test]
fn glyph_point_classification_and_trailing_contours_survive_both_backends() {
    let limits = Limits::default();
    let mut path = PathBuilder::new();
    path.move_to(Point::new(0.0, 0.0));
    path.line_to(Point::new(0.0, 1.0)).unwrap();
    path.close();
    path.move_to(Point::new(7.0, 7.0));
    // This singular linear map collapses the source controls, so the first
    // contour is a genuine point. The final open point still paints nothing.
    let geometry =
        Arc::new(GlyphGeometry::new(&path.finish(), [1.0, 0.0, 0.0, 0.0], &limits).unwrap());
    assert!(geometry.contours()[0].is_degenerate());
    for origin in [(0, 0), (17, 29), (1 << 30, 1 << 30)] {
        let shape = Shape {
            geometry: Geometry::Glyph(Arc::clone(&geometry)),
            transform: Transform::translate(f64::from(origin.0) + 2.25, f64::from(origin.1) + 3.75),
            rule: FillRule::NonZero,
        };
        for render in [coverage, coverage_accumulated] {
            let mut output = [0; 64];
            render(
                8,
                8,
                std::slice::from_ref(&shape),
                &mut output,
                origin,
                &limits,
                &mut Scratch::default(),
            )
            .unwrap();
            let mut expected = [0; 64];
            expected[3 * 8 + 2] = 255;
            assert_eq!(output, expected);
            let mut pixel = [0];
            render(
                1,
                1,
                std::slice::from_ref(&shape),
                &mut pixel,
                (origin.0 + 2, origin.1 + 3),
                &limits,
                &mut Scratch::default(),
            )
            .unwrap();
            assert_eq!(pixel, [255]);
            let restricted = Limits {
                path_segments: geometry.vertices() - 1,
                ..limits
            };
            assert_eq!(
                render(
                    8,
                    8,
                    std::slice::from_ref(&shape),
                    &mut output,
                    origin,
                    &restricted,
                    &mut Scratch::default()
                )
                .unwrap_err()
                .kind,
                pagura_core::ErrorKind::Limit
            );
        }
    }
}

#[test]
fn coincident_glyph_approximation_vertices_do_not_create_a_floor_cell() {
    let limits = Limits::default();
    let mut path = PathBuilder::new();
    let p = Point::new(4.0, 4.0);
    path.move_to(p);
    path.cubic_to(Point::new(4.0001, 4.0), Point::new(4.0, 4.0001), p)
        .unwrap();
    path.close();
    let glyph =
        Arc::new(GlyphGeometry::new(&path.finish(), [1.0, 0.0, 0.0, 1.0], &limits).unwrap());
    assert!(!glyph.contours()[0].is_degenerate());
    assert!(glyph.contours()[0].points().iter().all(|point| *point == p));
    let shapes = [Shape {
        geometry: Geometry::Glyph(glyph),
        transform: Transform::IDENTITY,
        rule: FillRule::NonZero,
    }];
    for render in [coverage, coverage_accumulated] {
        let mut output = [255; 64];
        render(
            8,
            8,
            &shapes,
            &mut output,
            (0, 0),
            &limits,
            &mut Scratch::default(),
        )
        .unwrap();
        assert_eq!(output, [0; 64]);
    }
}
