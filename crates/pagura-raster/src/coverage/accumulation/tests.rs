use super::*;
use crate::{FillRule, PathBuilder, Point, Transform};
use std::sync::Arc;

fn compare(points: &[Point], rule: FillRule) {
    let mut path = PathBuilder::new();
    path.move_to(points[0]);
    for &p in &points[1..] {
        path.line_to(p).unwrap();
    }
    path.close();
    let shapes = [Shape {
        geometry: crate::coverage::Geometry::Path(Arc::new(path.finish())),
        transform: Transform::IDENTITY,
        rule,
    }];
    compare_shapes(&shapes, (0, 0), true);
}

fn compare_shapes(shapes: &[Shape], origin: (u32, u32), tiles: bool) {
    let mut exact = vec![0; 64];
    let mut candidate = vec![0; 64];
    let limits = Limits::default();
    super::super::coverage_exact(
        crate::IntRect::new(origin.0, origin.1, 8, 8).unwrap(),
        shapes,
        &mut exact,
        &limits,
        &mut super::super::Exact::default(),
    )
    .unwrap();
    let mut accumulator = Accumulator::default();
    accumulator
        .render(shapes, &mut candidate, origin, (8, 8), &limits, 0)
        .unwrap();
    for (i, (&a, &b)) in exact.iter().zip(&candidate).enumerate() {
        assert!(
            a.abs_diff(b) <= 1,
            "{i}: sweep {a} accumulation {b}; origin {origin:?}, {shapes:?}"
        );
    }
    if tiles {
        for y in 0..8 {
            for x in 0..8 {
                let mut pixel = [0];
                accumulator
                    .render(
                        shapes,
                        &mut pixel,
                        (origin.0 + x, origin.1 + y),
                        (1, 1),
                        &limits,
                        0,
                    )
                    .unwrap();
                assert_eq!(pixel[0], candidate[(y * 8 + x) as usize], "tile {x},{y}");
            }
        }
    }
    assert!(accumulator.retained_bytes() > 0);
}

#[test]
fn certificate_resolver_and_tile_identity_on_random_self_intersections() {
    let mut seed = 192_873_u64;
    for case in 0..300 {
        let points: Vec<_> = (0..3 + case % 9)
            .map(|_| {
                let mut coordinate = || {
                    seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                    f64::from(u32::try_from(seed >> 32).unwrap() % 2049) / 256.0 - 0.5
                };
                Point::new(coordinate(), coordinate())
            })
            .collect();
        for rule in [FillRule::NonZero, FillRule::EvenOdd] {
            compare(&points, rule);
        }
    }
}

fn tile_boundaries(step: u32, phase: u32) -> Vec<u32> {
    let mut result = vec![0];
    let mut at = if phase == 0 { step } else { phase };
    while at < 8 {
        result.push(at);
        at += step;
    }
    result.push(8);
    result
}

fn all_tile_phases(shapes: &[Shape], origin: (u32, u32)) {
    let mut accumulator = Accumulator::default();
    let limits = Limits::default();
    let mut whole = [0; 64];
    accumulator
        .render(shapes, &mut whole, origin, (8, 8), &limits, 0)
        .unwrap();
    for (width, height) in [(1, 1), (2, 3), (7, 5)] {
        for py in 0..height {
            for px in 0..width {
                let xs = tile_boundaries(width, px);
                let ys = tile_boundaries(height, py);
                for y in ys.windows(2) {
                    for x in xs.windows(2) {
                        let (w, h) = (x[1] - x[0], y[1] - y[0]);
                        let mut tile = vec![0; (w * h) as usize];
                        accumulator
                            .render(
                                shapes,
                                &mut tile,
                                (origin.0 + x[0], origin.1 + y[0]),
                                (w, h),
                                &limits,
                                0,
                            )
                            .unwrap();
                        for j in 0..h {
                            for i in 0..w {
                                assert_eq!(
                                    tile[(j * w + i) as usize],
                                    whole[((y[0] + j) * 8 + x[0] + i) as usize],
                                    "tile phase {px},{py}, origin {origin:?}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn every_phase_of_small_tiles_preserves_groups_ghosts_and_translation() {
    for origin in [(0, 0), (17, 29), (1 << 30, 1 << 30)] {
        let transform = Transform::translate(f64::from(origin.0), f64::from(origin.1));
        let mut p = PathBuilder::new();
        p.move_to(Point::new(-1.0, 0.125));
        for (x, y) in [(7.75, 7.75), (0.25, 6.75), (7.25, -0.5)] {
            p.line_to(Point::new(x, y)).unwrap();
        }
        p.close();
        let fill = Arc::new(p.finish());
        p.rect(0.5, 0.25, 6.75, 6.875);
        let clip = Arc::new(p.finish());
        for rule in [FillRule::NonZero, FillRule::EvenOdd] {
            all_tile_phases(
                &[
                    Shape {
                        geometry: crate::coverage::Geometry::Path(Arc::clone(&fill)),
                        transform,
                        rule,
                    },
                    Shape {
                        geometry: crate::coverage::Geometry::Path(Arc::clone(&clip)),
                        transform,
                        rule: FillRule::NonZero,
                    },
                ],
                origin,
            );
        }
        for count in [127, 128, 129] {
            for i in 0..count {
                let lo = 2.0 + f64::from(4 * i + 2) / 1_048_576.0 - 0.25 / 1_048_576.0;
                p.rect(2.125, lo, 0.75, 0.5 / 1_048_576.0);
            }
            all_tile_phases(
                &[Shape {
                    geometry: crate::coverage::Geometry::Path(Arc::new(p.finish())),
                    transform,
                    rule: FillRule::NonZero,
                }],
                origin,
            );
        }
    }
}

fn next(seed: &mut u64) -> u32 {
    *seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
    u32::try_from(*seed >> 32).unwrap()
}

#[test]
fn random_contours_and_mixed_rule_groups_match_the_sweep_and_tiles() {
    let mut seed = 3_916_313_u64;
    for case in 0..1024 {
        let origin = [(0, 0), (17, 29), (1 << 30, 1 << 30)][case % 3];
        let mut shapes = Vec::new();
        for group in 0..2 + next(&mut seed) % 3 {
            let mut builder = PathBuilder::new();
            for contour in 0..2 + next(&mut seed) % 4 {
                if (case + contour as usize).is_multiple_of(17) {
                    let lo = 2.0 + 1.75 / 1_048_576.0;
                    if case % 2 == 0 {
                        builder.rect(1.125, lo, 5.75, 0.5 / 1_048_576.0);
                    } else {
                        builder.rect(lo, 1.125, 0.5 / 1_048_576.0, 5.75);
                    }
                    continue;
                }
                for vertex in 0..3 + next(&mut seed) % 10 {
                    let mut coordinate = || {
                        if case % 3 == 0 {
                            f64::from(next(&mut seed) % 11) - 1.0
                                + (f64::from(next(&mut seed) % 7) - 3.0) / 4_194_304.0
                        } else {
                            f64::from(next(&mut seed) % 2561) / 256.0 - 1.0
                        }
                    };
                    let p = Point::new(coordinate(), coordinate());
                    if vertex == 0 {
                        builder.move_to(p);
                    } else {
                        builder.line_to(p).unwrap();
                    }
                }
                builder.close();
            }
            shapes.push(Shape {
                geometry: crate::coverage::Geometry::Path(Arc::new(builder.finish())),
                transform: Transform::translate(
                    f64::from(origin.0) + f64::from(group) / 8.0,
                    f64::from(origin.1) - f64::from(group) / 16.0,
                ),
                rule: if (case + group as usize).is_multiple_of(2) {
                    FillRule::NonZero
                } else {
                    FillRule::EvenOdd
                },
            });
        }
        compare_shapes(&shapes, origin, case % 8 == 0);
        if case % 128 == 0 {
            all_tile_phases(&shapes, origin);
        }
        // Cached glyph polygons and paths must intersect as ordinary winding groups.
        for shape in shapes.iter_mut().step_by(2) {
            let crate::coverage::Geometry::Path(path) = &shape.geometry else {
                unreachable!()
            };
            shape.geometry = crate::coverage::Geometry::Glyph(Arc::new(
                crate::GlyphGeometry::new(
                    path,
                    Transform::IDENTITY,
                    &Limits::default(),
                    usize::MAX,
                )
                .unwrap(),
            ));
        }
        compare_shapes(&shapes, origin, case % 8 == 0);
        if case % 128 == 0 {
            all_tile_phases(&shapes, origin);
        }
    }
}
