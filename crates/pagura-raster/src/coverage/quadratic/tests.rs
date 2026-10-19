use super::*;
use crate::coverage::{Geometry, Scratch, Shape, coverage};
use crate::{FillRule, GlyphGeometry, PathBuilder, Transform};
use pagura_core::{ErrorKind, Limits};
use std::sync::Arc;

fn bowl() -> crate::Path {
    let mut path = PathBuilder::new();
    path.move_to(Point::new(0.0, 0.0));
    path.cubic_to(
        Point::new(1.0 / 3.0, 0.0),
        Point::new(2.0 / 3.0, 1.0 / 3.0),
        Point::new(1.0, 1.0),
    )
    .unwrap();
    path.line_to(Point::new(0.0, 1.0)).unwrap();
    path.close();
    path.finish()
}

fn glyph(path: &crate::Path) -> Arc<GlyphGeometry> {
    Arc::new(GlyphGeometry::new(path, [1.0, 0.0, 0.0, 1.0], &Limits::default()).unwrap())
}

fn shape(geometry: Arc<GlyphGeometry>, transform: Transform, rule: FillRule) -> Shape {
    Shape {
        geometry: Geometry::Glyph(geometry),
        transform,
        rule,
    }
}

#[test]
fn source_segments_and_preparation_work_are_bounded() {
    let mut path = PathBuilder::new();
    for _ in 0..4 {
        path.move_to(Point::new(-10.0, -10.0));
    }
    let error = build::prepare(
        &path.finish(),
        Transform::IDENTITY,
        &Limits {
            path_segments: 3,
            ..Limits::default()
        },
        usize::MAX,
    )
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
    assert_eq!(error.message, "path segment budget exceeded");
    let limits = Limits {
        raster_work: 1,
        ..Limits::default()
    };
    assert_eq!(
        build::prepare(&bowl(), Transform::IDENTITY, &limits, usize::MAX)
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
}

#[test]
fn preparation_stops_once_the_admission_cap_is_exceeded() {
    // One monotone parabola and its vertical closing edge.
    let limits = Limits::default();
    let pieces = build::prepare(&bowl(), Transform::IDENTITY, &limits, 2).unwrap();
    assert_eq!(pieces.map(|p| p.len()), Some(2));
    assert!(
        build::prepare(&bowl(), Transform::IDENTITY, &limits, 1)
            .unwrap()
            .is_none()
    );
}

// Cubic edges from `from` to `to` with their controls moved by `offset`
// across the chord, as -D/6 and D/6 for a third difference D.
fn wiggle(path: &mut PathBuilder, from: Point, to: Point, offset: f64) {
    let third = Point::new((to.x - from.x) / 3.0, (to.y - from.y) / 3.0);
    path.cubic_to(
        Point::new(from.x + third.x - offset, from.y + third.y + offset),
        Point::new(to.x - third.x + offset, to.y - third.y - offset),
        to,
    )
    .unwrap();
}

#[test]
fn only_outlines_of_quadratic_curves_are_admitted() {
    // Two slivers within one pixel column, edged by cubics just below the
    // displacement at which a midpoint quadratic would split (1/1024 pixel).
    // Their displacements add: one quadratic each put a pixel 1.6 levels from
    // the true curve, so general cubics keep their polygons.
    let outline = |offset: f64| {
        let mut path = PathBuilder::new();
        path.move_to(Point::new(30.0, 40.0));
        path.cubic_to(
            Point::new(40.0, 200.0 / 3.0),
            Point::new(50.0, 200.0 / 3.0),
            Point::new(60.0, 40.0),
        )
        .unwrap();
        path.close();
        for (x, width) in [(0.0, 0.1375), (0.3875, 0.1375)] {
            path.move_to(Point::new(x, 0.0));
            wiggle(
                &mut path,
                Point::new(x, 0.0),
                Point::new(x + 20.0, 20.0),
                offset,
            );
            path.line_to(Point::new(x + width + 20.0, 20.0)).unwrap();
            let end = Point::new(x + width, 0.0);
            wiggle(&mut path, Point::new(x + width + 20.0, 20.0), end, offset);
            path.close();
        }
        glyph(&path.finish())
    };
    assert!(outline(0.0).curves().is_some());
    assert!(outline(0.0202 / 6.0 / 2.0_f64.sqrt()).curves().is_none());
}

#[test]
fn inverse_and_integral_cover_flat_tangents_and_reversed_x() {
    for points in [
        [
            Point::new(0.0, 0.0),
            Point::new(0.0, 0.0),
            Point::new(1.0, 1.0),
        ],
        [
            Point::new(1.0, 0.0),
            Point::new(1.0, 0.0),
            Point::new(0.0, 1.0),
        ],
        [
            Point::new(0.0, 0.0),
            Point::new(1.0, 1.0),
            Point::new(1.0, 1.0),
        ],
    ] {
        let curve = Quadratic { points };
        for i in 0..=100 {
            let t = f64::from(i) / 100.0;
            let p = curve.at(t);
            assert!((curve.parameter(p.y, false) - t).abs() < 1e-12);
            assert!((curve.parameter(p.x, true) - t).abs() < 1e-12);
        }
        assert!((curve.cell(0.0, 1.0, 0.0) - 0.5).abs() < 1e-12);
    }
}

#[test]
fn integration_orders_the_parameters_of_nearly_equal_heights() {
    // From a hostile outline: rounding inverts these parameters, and clamping
    // a cell between them once panicked.
    let curve = Quadratic {
        points: [
            Point::new(-2.548_484_830_085_885, -1.488_990_183_360_162_9),
            Point::new(4.286_554_604_663_946, 0.785_060_039_657_866_4),
            Point::new(4.286_554_604_663_946, 3.588_751_331_853_294),
        ],
    };
    let (lo, hi) = (0.433_209_056_223_925_47, 0.433_209_056_223_925_7);
    assert!(curve.parameter(lo, false) > curve.parameter(hi, false));
    let mut row = Row {
        maximum: 1000,
        ..Row::default()
    };
    row.reset(8, 0).unwrap();
    integrate(&mut row, curve, lo, hi, 1).unwrap();
}

#[test]
fn cached_parabolic_glyph_has_exact_two_thirds_area() {
    // x=t, y=t². Integral of 1-x² over [0,1] is 2/3, or 170 levels.
    let geometry = glyph(&bowl());
    assert!(geometry.curves().is_some());
    for rule in [FillRule::NonZero, FillRule::EvenOdd] {
        let shapes = [shape(Arc::clone(&geometry), Transform::IDENTITY, rule)];
        let mut out = [0];
        coverage(
            1,
            1,
            &shapes,
            &mut out,
            (0, 0),
            &Limits::default(),
            &mut Scratch::default(),
        )
        .unwrap();
        assert_eq!(out, [170]);
    }
}

#[test]
fn nonrectangular_clip_preserves_polygon_backend_even_when_it_covers_the_tile() {
    let mut path = PathBuilder::new();
    path.move_to(Point::new(-10.0, -10.0));
    path.line_to(Point::new(20.0, -10.0)).unwrap();
    path.line_to(Point::new(-10.0, 20.0)).unwrap();
    path.close();
    let shapes = [
        shape(glyph(&bowl()), Transform::IDENTITY, FillRule::NonZero),
        Shape {
            geometry: Geometry::Path(Arc::new(path.finish())),
            transform: Transform::IDENTITY,
            rule: FillRule::NonZero,
        },
    ];
    assert!(anchor(&shapes).is_none());
    let mut actual = [0];
    let mut expected = [0];
    coverage(
        1,
        1,
        &shapes,
        &mut actual,
        (0, 0),
        &Limits::default(),
        &mut Scratch::default(),
    )
    .unwrap();
    crate::coverage::coverage_accumulated(
        1,
        1,
        &shapes,
        &mut expected,
        (0, 0),
        &Limits::default(),
        &mut Scratch::default(),
    )
    .unwrap();
    assert_eq!(actual, expected);
    assert!(
        anchor(&[Shape {
            geometry: Geometry::Path(Arc::new(bowl())),
            transform: Transform::IDENTITY,
            rule: FillRule::NonZero
        }])
        .is_none()
    );
}

#[test]
fn cached_curves_keep_exact_placement_across_tiles_and_large_integer_translations() {
    let mut path = PathBuilder::new();
    path.move_to(Point::new(0.25, 0.25));
    path.cubic_to(
        Point::new(2.75, -1.25),
        Point::new(4.5, 0.5),
        Point::new(5.5, 5.5),
    )
    .unwrap();
    path.line_to(Point::new(0.25, 5.5)).unwrap();
    path.close();
    let geometry = glyph(&path.finish());
    assert!(geometry.curves().is_some());
    for offset in [0, 1_000_000_000] {
        let shapes = [shape(
            Arc::clone(&geometry),
            Transform::translate(f64::from(offset), f64::from(offset)),
            FillRule::NonZero,
        )];
        let mut full = [0; 36];
        coverage(
            6,
            6,
            &shapes,
            &mut full,
            (offset, offset),
            &Limits::default(),
            &mut Scratch::default(),
        )
        .unwrap();
        for y in 0..6 {
            for x in 0..6 {
                let mut pixel = [0];
                coverage(
                    1,
                    1,
                    &shapes,
                    &mut pixel,
                    (offset + x, offset + y),
                    &Limits::default(),
                    &mut Scratch::default(),
                )
                .unwrap();
                assert_eq!(pixel[0], full[(y * 6 + x) as usize]);
            }
        }
    }
}

#[test]
fn point_contours_choose_the_floor_cell_after_placement() {
    let mut path = PathBuilder::new();
    for segment in bowl().segments() {
        match *segment {
            crate::Segment::MoveTo(p) => {
                path.move_to(p);
            }
            crate::Segment::LineTo(p) => {
                path.line_to(p).unwrap();
            }
            crate::Segment::CubicTo(a, b, c) => {
                path.cubic_to(a, b, c).unwrap();
            }
            crate::Segment::Close => {
                path.close();
            }
        }
    }
    path.move_to(Point::new(1.25, 1.25));
    path.close();
    let geometry = glyph(&path.finish());
    assert!(geometry.curves().is_none());
    for phase in [0.0, 0.25, 0.75] {
        let shapes = [shape(
            Arc::clone(&geometry),
            Transform::translate(phase, phase),
            FillRule::NonZero,
        )];
        let mut out = [0; 9];
        coverage(
            3,
            3,
            &shapes,
            &mut out,
            (0, 0),
            &Limits::default(),
            &mut Scratch::default(),
        )
        .unwrap();
        let cell = if phase < 0.75 { 4 } else { 8 };
        assert_eq!(out[cell], 255);
    }
}

// A quadratic arc, elevated to the cubic a glyph outline stores.
fn quadratic_to(path: &mut PathBuilder, from: Point, control: Point, to: Point) {
    let lift = |end: Point| {
        Point::new(
            end.x + (control.x - end.x) * (2.0 / 3.0),
            end.y + (control.y - end.y) * (2.0 / 3.0),
        )
    };
    path.cubic_to(lift(from), lift(to), to).unwrap();
}

// Eight quadratic arcs on a 1/8-pixel grid, as a TrueType ring.
fn ring(path: &mut PathBuilder, radius: f64, diagonal: f64, handle: f64, reverse: bool) {
    let (r, d, h) = (radius, diagonal, handle);
    let on = [
        (r, 0.0),
        (d, d),
        (0.0, r),
        (-d, d),
        (-r, 0.0),
        (-d, -d),
        (0.0, -r),
        (d, -d),
    ];
    let off = [
        (r, h),
        (h, r),
        (-h, r),
        (-r, h),
        (-r, -h),
        (-h, -r),
        (h, -r),
        (r, -h),
    ];
    let point = |(x, y): (f64, f64)| Point::new(x, y);
    let mut arcs: Vec<[Point; 3]> = (0..8)
        .map(|k| [point(on[k]), point(off[k]), point(on[(k + 1) % 8])])
        .collect();
    if reverse {
        arcs.reverse();
        for arc in &mut arcs {
            arc.reverse();
        }
    }
    path.move_to(arcs[0][0]);
    for [from, control, to] in arcs {
        quadratic_to(path, from, control, to);
    }
    path.close();
}

#[test]
fn crossings_beside_the_other_parabola_branch_are_found_in_every_tile() {
    // A crossbar over a bowl, as overlapping TrueType contours draw it. Near
    // this pixel the bar also meets the other branch of an arc's parabola, so
    // the genuine crossing is a near-double root of the elimination.
    let mut path = PathBuilder::new();
    ring(&mut path, 11.0, 7.75, 4.5, false);
    ring(&mut path, 8.75, 6.125, 3.625, true);
    path.move_to(Point::new(-14.25, 9.75));
    path.line_to(Point::new(14.25, 7.25)).unwrap();
    path.line_to(Point::new(14.25, 8.125)).unwrap();
    path.line_to(Point::new(-14.25, 10.625)).unwrap();
    path.close();
    let geometry = glyph(&path.finish());
    assert!(geometry.curves().is_some());
    let shapes = [shape(
        geometry,
        Transform::translate(18.625, 18.75),
        FillRule::NonZero,
    )];
    // Independent scanline quadrature of the true outline: 166.660 levels.
    let (x, y) = (17_u32, 27_u32);
    for ((left, top), size) in [((0, 0), 36), ((x, y), 1), ((14, 24), 7), ((16, 16), 16)] {
        let mut out = vec![0; (size * size) as usize];
        coverage(
            size,
            size,
            &shapes,
            &mut out,
            (left, top),
            &Limits::default(),
            &mut Scratch::default(),
        )
        .unwrap();
        let value = out[((y - top) * size + x - left) as usize];
        assert!(
            (f64::from(value) - 166.660).abs() <= 0.5,
            "tile at {left},{top}: {value}"
        );
    }
}
