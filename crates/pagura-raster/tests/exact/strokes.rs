//! Strokes against the exact area of their outlines.

use crate::support::{Lcg, area, compare, invariant, options, pixels, raster, reference};
use pagura_core::Limits;
use pagura_raster::{
    Adjustment, Area, ClipShape, LineCap, LineJoin, Path, PathBuilder, Point, Shape, ShapeOptions,
    Stroke, Transform,
};
use pagura_reference::exact;

/// A stroke class's differences from the exact coverage: (strokes, pixels
/// compared, pixels differing, largest difference).
#[derive(Debug, Default, Clone, Copy)]
struct Tally {
    strokes: usize,
    pixels: usize,
    differing: usize,
    largest: u8,
}

/// Compares a stroke of `path` with `stroke` under the pen space to user
/// space map `ctm` and the device transform `transform`, checking
/// invariance across partitions where `partitions` asks for it.
fn check(
    area: &Area,
    path: &Path,
    stroke: &Stroke,
    (ctm, transform): (Transform, Transform),
    (adjustment, partitions): (Adjustment, bool),
    tally: &mut Tally,
) {
    let limits = Limits::default();
    let pen = ctm.inverse().unwrap();
    let prepare = |options: &ShapeOptions<'_>, clip: Option<&ClipShape>| {
        Shape::stroke_with_clip(path, stroke, (pen, transform), adjustment, clip, options).unwrap()
    };
    let new = if partitions {
        invariant(area, &prepare, &[])
    } else {
        prepare(&options(*area, &limits), None).map_or_else(
            || vec![0; pixels(area.region)],
            |s| raster(area, Some(&s), None).0,
        )
    };
    let mark = exact::stroke(path, stroke, ctm, adjustment, transform, &limits).unwrap();
    let (count, largest) = compare(&new, &reference(area.region, mark, &[]));
    tally.strokes += 1;
    tally.pixels += new.len();
    tally.differing += count;
    tally.largest = tally.largest.max(largest);
}

#[test]
fn round_cap_keeps_the_canonical_polygon_near_a_half_level() {
    let limits = Limits::default();
    let area = area(0, 0, 3, 2, 32);
    let transform = Transform::new(25.0 / 6.0, 0.0, 0.0, -25.0 / 6.0, -652.0, 823.0);
    let stroke = Stroke {
        width: 1.0,
        cap: LineCap::Round,
        ..Stroke::default()
    };
    for points in [
        [(156.83, 207.0), (156.83, 198.0)],
        [(156.83, 198.0), (156.83, 207.0)],
    ] {
        let mut path = PathBuilder::new();
        path.move_to(Point::new(points[0].0, points[0].1));
        path.line_to(Point::new(points[1].0, points[1].1)).unwrap();
        let path = path.finish();
        let mark = exact::stroke(
            &path,
            &stroke,
            Transform::IDENTITY,
            Adjustment::None,
            transform,
            &limits,
        )
        .unwrap();
        let expected = reference(area.region, mark, &[]);
        // Exact Fraction clipping of the canonical stored circle polygon
        // gives 0.5000447760 levels here. A separately sampled half disc
        // gives 0.4864086785, dropping a coverage byte before composition.
        assert_eq!(expected[0], 1);
        let prepare = |options: &ShapeOptions<'_>, clip: Option<&ClipShape>| {
            Shape::stroke_with_clip(
                &path,
                &stroke,
                (Transform::IDENTITY, transform),
                Adjustment::None,
                clip,
                options,
            )
            .unwrap()
        };
        assert_eq!(invariant(&area, &prepare, &[]), expected);
    }
}

#[test]
fn round_cap_polygons_match_for_reversed_rotated_and_dashed_lines() {
    let limits = Limits::default();
    let area = area(0, 0, 48, 48, 16);
    for transform in [
        Transform::IDENTITY,
        Transform::new(0.8, 0.6, -0.6, 0.8, 20.0, 1.0),
        Transform::new(1.2, 0.3, 0.2, 0.7, 1.0, 2.0),
    ] {
        for points in [[(5.3, 6.2), (16.7, 17.3)], [(16.7, 17.3), (5.3, 6.2)]] {
            let mut path = PathBuilder::new();
            path.move_to(Point::new(points[0].0, points[0].1));
            path.line_to(Point::new(points[1].0, points[1].1)).unwrap();
            let path = path.finish();
            for dash_array in [vec![], vec![3.0, 2.0], vec![0.0, 3.0]] {
                let stroke = Stroke {
                    width: 1.25,
                    cap: LineCap::Round,
                    dash_array,
                    ..Stroke::default()
                };
                let mark = exact::stroke(
                    &path,
                    &stroke,
                    Transform::IDENTITY,
                    Adjustment::None,
                    transform,
                    &limits,
                )
                .unwrap();
                let shape = Shape::stroke(
                    &path,
                    &stroke,
                    Transform::IDENTITY,
                    transform,
                    Adjustment::None,
                    &options(area, &limits),
                )
                .unwrap()
                .unwrap();
                assert_eq!(
                    raster(&area, Some(&shape), None).0,
                    reference(area.region, mark, &[]),
                    "{points:?}, {transform:?}, {stroke:?}"
                );
            }
        }
    }
}

#[test]
fn round_cap_slabs_keep_clip_and_band_geometry() {
    let limits = Limits::default();
    let area = area(0, 0, 32, 128, 16);
    let mut path = PathBuilder::new();
    path.move_to(Point::new(8.25, 5.1));
    path.line_to(Point::new(8.25, 105.3)).unwrap();
    let path = path.finish();
    let stroke = Stroke {
        width: 4.1,
        cap: LineCap::Round,
        ..Stroke::default()
    };
    let mut clip = PathBuilder::new();
    clip.polygon([
        Point::new(6.1, 0.0),
        Point::new(17.0, 0.0),
        Point::new(14.5, 128.0),
        Point::new(7.9, 128.0),
    ]);
    let clip = clip.finish();
    let clips = [(&clip, pagura_raster::FillRule::NonZero)];
    let prepare = |options: &ShapeOptions<'_>, clip: Option<&ClipShape>| {
        Shape::stroke_with_clip(
            &path,
            &stroke,
            (Transform::IDENTITY, Transform::IDENTITY),
            Adjustment::None,
            clip,
            options,
        )
        .unwrap()
    };
    let mark = exact::stroke(
        &path,
        &stroke,
        Transform::IDENTITY,
        Adjustment::None,
        Transform::IDENTITY,
        &limits,
    )
    .unwrap();
    assert_eq!(
        invariant(&area, &prepare, &clips),
        reference(area.region, mark, &clips)
    );
}

fn paths(rng: &mut Lcg) -> Vec<Path> {
    let mut out = Vec::new();
    // Two-segment polylines, as most marks of dense vector pages are.
    for _ in 0..6 {
        let p = |rng: &mut Lcg| Point::new(rng.grid(4.0, 36.0, 10), rng.grid(4.0, 28.0, 10));
        let mut path = PathBuilder::new();
        path.move_to(p(rng));
        path.line_to(p(rng)).unwrap();
        path.line_to(p(rng)).unwrap();
        out.push(path.finish());
    }
    // Axis-aligned rules, a closed square, a curve, a single point.
    let mut path = PathBuilder::new();
    path.move_to(Point::new(3.0, 10.3));
    path.line_to(Point::new(37.0, 10.3)).unwrap();
    path.move_to(Point::new(20.5, 2.0));
    path.line_to(Point::new(20.5, 30.0)).unwrap();
    out.push(path.finish());
    let mut path = PathBuilder::new();
    path.rect(8.25, 6.5, 20.0, 15.0);
    out.push(path.finish());
    let mut path = PathBuilder::new();
    path.move_to(Point::new(5.0, 25.0));
    path.cubic_to(
        Point::new(12.0, 2.0),
        Point::new(30.0, 40.0),
        Point::new(36.0, 8.0),
    )
    .unwrap();
    out.push(path.finish());
    let mut path = PathBuilder::new();
    path.move_to(Point::new(17.5, 17.5));
    path.close();
    out.push(path.finish());
    // Runs whose directions span more than a half turn: a letter-like
    // hook, a closed non-convex outline, a curve that turns back near
    // itself and a zigzag.
    let polyline = |points: &[(f64, f64)], closed: bool| {
        let mut path = PathBuilder::new();
        path.move_to(Point::new(points[0].0, points[0].1));
        for &(x, y) in &points[1..] {
            path.line_to(Point::new(x, y)).unwrap();
        }
        if closed {
            path.close();
        }
        path.finish()
    };
    out.push(polyline(
        &[
            (6.0, 6.0),
            (6.0, 20.0),
            (8.1, 24.2),
            (12.0, 25.3),
            (16.2, 24.1),
            (18.0, 20.0),
            (18.0, 6.5),
        ],
        false,
    ));
    out.push(polyline(
        &[
            (24.0, 4.0),
            (38.0, 4.5),
            (38.0, 12.0),
            (30.2, 12.3),
            (30.0, 26.0),
            (24.3, 26.0),
        ],
        true,
    ));
    let mut path = PathBuilder::new();
    path.move_to(Point::new(8.0, 30.0));
    path.cubic_to(
        Point::new(40.0, 34.0),
        Point::new(36.0, 4.0),
        Point::new(20.0, 14.0),
    )
    .unwrap();
    path.cubic_to(
        Point::new(10.0, 20.0),
        Point::new(16.0, 31.0),
        Point::new(26.5, 27.5),
    )
    .unwrap();
    out.push(path.finish());
    out.push(polyline(
        &[
            (4.0, 8.0),
            (10.0, 16.0),
            (16.0, 8.5),
            (22.0, 16.5),
            (28.0, 9.0),
            (34.0, 17.0),
            (20.0, 30.0),
            (5.0, 24.0),
        ],
        false,
    ));
    out
}

#[test]
fn strokes_of_every_class_are_within_one_level() {
    let mut rng = Lcg(5150);
    let area = area(0, 0, 44, 36, 16);
    let paths = paths(&mut rng);
    let transforms = [
        (Transform::IDENTITY, Transform::IDENTITY),
        (Transform::scale(0.5, 0.5), Transform::scale(2.0, 2.0)),
        (
            Transform::new(1.0, 0.0, 0.3, 0.8, 0.0, 0.0),
            Transform::IDENTITY,
        ),
    ];
    let mut tallies = std::collections::BTreeMap::new();
    for cap in [LineCap::Butt, LineCap::ProjectingSquare, LineCap::Round] {
        for join in [LineJoin::Miter, LineJoin::Bevel, LineJoin::Round] {
            for width in [0.0, 0.6, 1.0, 3.7] {
                for dashes in [vec![], vec![2.0, 1.5]] {
                    let stroke = Stroke {
                        width,
                        cap,
                        join,
                        miter_limit: if width > 1.0 { 2.0 } else { 10.0 },
                        dash_array: dashes.clone(),
                        dash_phase: 0.75,
                    };
                    for (t, &transform) in transforms.iter().enumerate() {
                        for path in &paths {
                            for adjustment in [Adjustment::None, Adjustment::Adjusted] {
                                let class = (
                                    format!("{cap:?}/{join:?}"),
                                    adjustment == Adjustment::Adjusted,
                                );
                                let tally: &mut Tally = tallies.entry(class).or_default();
                                let partitions = t == 0 && width != 0.6;
                                check(
                                    &area,
                                    path,
                                    &stroke,
                                    transform,
                                    (adjustment, partitions),
                                    tally,
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    for ((class, adjusted), tally) in &tallies {
        eprintln!("strokes {class} adjusted={adjusted}: {tally:?}");
        assert!(tally.largest <= 1, "{class}: {tally:?}");
    }
}

/// Tiny changes made in pen space can cover visible area after magnification.
#[test]
fn capsule_admission_bounds_rounding_after_pen_transform() {
    let limits = Limits::default();
    let area = area(0, 0, 48, 48, 16);
    let anchor = 34_359_738_368.0;
    let scale = 65_536.0;
    let mut path = PathBuilder::new();
    path.move_to(Point::new(anchor, anchor));
    path.line_to(Point::new(
        anchor + 49.0 / 131_072.0,
        anchor + 9.0 / 131_072.0,
    ))
    .unwrap();
    let path = path.finish();
    let stroke = Stroke {
        width: 8.0 / scale,
        cap: LineCap::Round,
        ..Stroke::default()
    };
    let transform = Transform::new(
        scale,
        0.0,
        0.0,
        scale,
        30.25 - anchor * scale,
        30.25 - anchor * scale,
    );
    let mark = exact::stroke(
        &path,
        &stroke,
        Transform::IDENTITY,
        Adjustment::None,
        transform,
        &limits,
    )
    .unwrap();
    let expected = reference(area.region, mark, &[]);
    assert_eq!(expected[26 * 48 + 31], 161);
    let prepare = |options: &ShapeOptions<'_>, clip: Option<&ClipShape>| {
        Shape::stroke_with_clip(
            &path,
            &stroke,
            (Transform::IDENTITY, transform),
            Adjustment::None,
            clip,
            options,
        )
        .unwrap()
    };
    assert_eq!(invariant(&area, &prepare, &[]), expected);
}
