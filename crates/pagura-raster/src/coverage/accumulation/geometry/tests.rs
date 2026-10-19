use super::*;
use crate::coverage::RASTER_WORK;
use crate::{PathBuilder, Transform};
use std::sync::Arc;

fn meter(maximum: usize) -> Meter {
    Meter::new(maximum, RASTER_WORK)
}

fn build(points: &[Point]) -> Geometry {
    let mut path = PathBuilder::new();
    path.move_to(points[0]);
    for &point in &points[1..] {
        path.line_to(point).unwrap();
    }
    path.close();
    let mut geometry = Geometry::default();
    geometry
        .build(
            &[Shape {
                geometry: crate::coverage::Geometry::Path(Arc::new(path.finish())),
                transform: Transform::IDENTITY,
                rule: FillRule::NonZero,
            }],
            &Limits::default(),
            &mut meter(usize::MAX),
        )
        .unwrap();
    geometry
}

#[test]
fn cyclic_chains_keep_the_run_across_the_contour_start() {
    let g = build(&[
        Point::new(0.0, 0.5),
        Point::new(0.0, 1.0),
        Point::new(1.0, 1.0),
        Point::new(1.0, 0.0),
        Point::new(0.0, 0.0),
    ]);
    let ids: Vec<_> = g.segments.iter().map(|s| s.chain).collect();
    assert_eq!(ids[0], ids[4]);
    assert_eq!(ids[0], ids[1]);
    assert_eq!(ids[2], ids[3]);
    assert_ne!(ids[0], ids[2]);
    assert!(g.neighbours(0, 4));
    assert!(!g.neighbours(0, 2));
    assert_eq!(g.group(4), 0);
    assert_eq!(g.next(4), 0);
    assert_eq!(g.contours[0].enclosed, Some([0, 0]));
    assert!(!g.contours[0].collapsed);
    assert_eq!(g.contours[0].bounds, [0.0, 0.0, 1.0, 1.0]);
    assert!(g.retained_bytes() > 0);
}

#[test]
fn collapsed_original_segments_and_contours_remain_visible() {
    let quarter = 0.25 / 1_048_576.0;
    for vertical in [false, true] {
        let mut p = [
            Point::new(0.125, 0.5 - quarter),
            Point::new(0.875, 0.5 - quarter),
            Point::new(0.875, 0.5 + quarter),
            Point::new(0.125, 0.5 + quarter),
        ];
        if vertical {
            for point in &mut p {
                std::mem::swap(&mut point.x, &mut point.y);
            }
        }
        let g = build(&p);
        assert_eq!(g.exact, p);
        assert_eq!(g.segments.len(), 4);
        assert!(g.contours[0].collapsed);
        assert_eq!(g.segments.iter().filter(|s| s.ghost).count(), 2);
    }
}

#[test]
fn chains_are_monotone_and_indices_remain_valid_under_random_geometry() {
    let mut seed = 381_173_u64;
    for case in 0..2000 {
        let points: Vec<_> = (0..3 + case % 19)
            .map(|_| {
                let mut coordinate = || {
                    seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                    f64::from(u32::try_from(seed >> 32).unwrap() % 65) / 16.0
                };
                Point::new(coordinate(), coordinate())
            })
            .collect();
        let g = build(&points);
        let mut directions = std::collections::BTreeMap::new();
        for (i, s) in g.segments.iter().enumerate() {
            let d = direction(g.points[i], g.points[g.next(i)]);
            if d != 0 {
                assert_eq!(*directions.entry(s.chain).or_insert(d), d);
            }
            assert_eq!(g.group(i), 0);
            assert!(g.neighbours(i, g.next(i)));
        }
    }
}

#[test]
fn huge_coordinates_and_budget_failures_are_explicit() {
    let g = build(&[
        Point::new(-1e12, -1e12),
        Point::new(1e12, 1e12),
        Point::new(1e12, -1e12),
    ]);
    assert!(g.points.iter().flatten().all(|v| v.abs() < 1_i64 << 60));
    let mut p = PathBuilder::new();
    p.rect(0.0, 0.0, 1.0, 1.0);
    let shapes = [Shape {
        geometry: crate::coverage::Geometry::Path(Arc::new(p.finish())),
        transform: Transform::IDENTITY,
        rule: FillRule::NonZero,
    }];
    let mut geometry = Geometry::default();
    assert!(
        geometry
            .build(&shapes, &Limits::default(), &mut meter(0))
            .is_err()
    );
    assert!(
        geometry
            .build(
                &shapes,
                &Limits {
                    mask_sources: 0,
                    ..Limits::default()
                },
                &mut meter(usize::MAX)
            )
            .is_err()
    );
    geometry
        .build(&shapes, &Limits::default(), &mut meter(usize::MAX))
        .unwrap();
    assert_eq!(geometry.points.len(), 4);
}
