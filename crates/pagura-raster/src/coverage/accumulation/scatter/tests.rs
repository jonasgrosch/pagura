use super::super::geometry::SCALE;
use super::*;
use crate::coverage::{RASTER_WORK, Shape};
use crate::{FillRule, PathBuilder, Point, Transform};
use pagura_core::Limits;
use std::collections::BTreeMap;
use std::sync::Arc;

fn geometry(points: &[Point], shift: u32) -> Geometry {
    let mut p = PathBuilder::new();
    p.move_to(points[0]);
    for &point in &points[1..] {
        p.line_to(point).unwrap();
    }
    p.close();
    let mut g = Geometry::default();
    g.build(
        &[Shape {
            geometry: crate::coverage::Geometry::Path(Arc::new(p.finish())),
            transform: Transform::translate(f64::from(shift), f64::from(shift)),
            rule: FillRule::NonZero,
        }],
        &Limits::default(),
        &mut meter(),
    )
    .unwrap();
    g
}

fn meter() -> Meter {
    Meter::new(Limits::default().raster_work, RASTER_WORK)
}

fn values(g: &Geometry, origin: (u32, u32), size: (u32, u32)) -> BTreeMap<(u32, u32), i128> {
    let mut scatter = Scatter::default();
    let mut work = meter();
    let area = IntRect::new(origin.0, origin.1, size.0, size.1).unwrap();
    scatter.prepare(g, area, &mut work).unwrap();
    let mut result = BTreeMap::new();
    for row in 0..size.1 {
        scatter.row(g, area, row, &mut work).unwrap();
        let mut prefix = scatter.carry[0];
        for column in 0..size.0 {
            let mut value = 2 * i128::from(GRID) * i128::from(prefix);
            for piece in scatter.pieces.iter().filter(|p| p.column == column) {
                let dy = i64::from(piece.b[1]) - i64::from(piece.a[1]);
                value += i128::from(2 * GRID - i64::from(piece.a[0]) - i64::from(piece.b[0]))
                    * i128::from(dy);
                prefix += dy;
            }
            result.insert((origin.0 + column, origin.1 + row), value);
        }
    }
    assert!(scatter.retained_bytes() > 0);
    result
}

#[test]
fn integral_of_winding_matches_analytic_dyadic_triangle() {
    let g = geometry(
        &[
            Point::new(0.25, 0.25),
            Point::new(3.75, 0.25),
            Point::new(0.25, 3.75),
        ],
        0,
    );
    let actual = values(&g, (0, 0), (5, 5));
    let denominator = 2 * i128::from(GRID).pow(2);
    // Ray-to-left winding is -1. The upper-left cell has a 3/4 square;
    // cells (1,1) and (2,1) are respectively full and half covered.
    assert_eq!(actual[&(0, 0)], -9 * denominator / 16);
    assert_eq!(actual[&(1, 1)], -denominator);
    assert_eq!(actual[&(2, 1)], -denominator / 2);
    assert_eq!(actual[&(4, 4)], 0);
}

#[test]
fn integer_values_match_every_tile_split_and_whole_pixel_translation() {
    let points = [
        Point::new(-2.75, 0.15),
        Point::new(6.8, 5.1),
        Point::new(1.2, -2.3),
        Point::new(4.6, 6.7),
        Point::new(-1.1, 3.3),
    ];
    for shift in [0, 17, 1 << 30] {
        let g = geometry(&points, shift);
        let full = values(&g, (shift, shift), (7, 7));
        for y in 0..7 {
            for x in 0..7 {
                for size in [(1, 1), (2, 3), (7 - x, 7 - y)] {
                    let tile = values(&g, (shift + x, shift + y), size);
                    for (key, value) in tile {
                        if let Some(&expected) = full.get(&key) {
                            assert_eq!(value, expected, "{key:?}");
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn original_density_and_ghosts_survive_the_512_piece_cutoff() {
    for count in [127, 128, 129, 512, 513] {
        for vertical in [false, true] {
            let mut p = PathBuilder::new();
            for i in 0..count {
                let lo = f64::from(4 * i + 2) / SCALE - 0.25 / SCALE;
                if vertical {
                    p.rect(lo, 0.125, 0.5 / SCALE, 0.75);
                } else {
                    p.rect(0.125, lo, 0.75, 0.5 / SCALE);
                }
            }
            let mut g = Geometry::default();
            let limits = Limits::default();
            let mut work = meter();
            g.build(
                &[Shape {
                    geometry: crate::coverage::Geometry::Path(Arc::new(p.finish())),
                    transform: Transform::IDENTITY,
                    rule: FillRule::NonZero,
                }],
                &limits,
                &mut work,
            )
            .unwrap();
            let mut scatter = Scatter::default();
            let area = IntRect::new(0, 0, 1, 1).unwrap();
            scatter.prepare(&g, area, &mut work).unwrap();
            scatter.row(&g, area, 0, &mut work).unwrap();
            let originals: i64 = scatter
                .risks
                .iter()
                .filter(|r| r.column == 0)
                .map(|r| r.originals)
                .sum();
            let ghosts: i64 = scatter
                .risks
                .iter()
                .filter(|r| r.column == 0)
                .map(|r| r.ghosts)
                .sum();
            let originals = originals + scatter.sparse_bound().unwrap_or(0);
            assert_eq!(originals, i64::from(4 * count));
            assert!(ghosts > 0);
            assert_eq!(originals > 512, count > 128);
        }
    }
}

#[test]
fn huge_endpoints_use_exact_closed_form_crossings_and_left_carry() {
    let g = geometry(
        &[
            Point::new(-1e12, -1e12),
            Point::new(1e12, 1e12),
            Point::new(1e12, -1e12),
        ],
        0,
    );
    let values = values(&g, (0, 0), (7, 5));
    let unit = 2 * i128::from(GRID).pow(2);
    assert_eq!(values[&(0, 0)], unit / 2);
    assert_eq!(values[&(1, 0)], unit);
    assert_eq!(values[&(0, 1)], 0);
    assert_eq!(interpolate([0, 0], [2, 1], 1, Axis::X), 1);
    assert_eq!(interpolate([0, 0], [2, -1], 1, Axis::X), 0);
}

#[test]
fn collapsed_contour_bounds_mark_every_closed_cell_they_touch() {
    let g = geometry(
        &[
            Point::new(1.0, 1.0),
            Point::new(2.0, 2.0),
            Point::new(3.0, 3.0),
        ],
        0,
    );
    assert!(g.contours[0].collapsed);
    let mut scatter = Scatter::default();
    let mut work = meter();
    let area = IntRect::new(0, 0, 4, 4).unwrap();
    scatter.prepare(&g, area, &mut work).unwrap();
    for row in 0..4 {
        scatter.row(&g, area, row, &mut work).unwrap();
        let mut ghosts = 0;
        for column in 0..4 {
            for event in scatter.risks.iter().filter(|e| e.column == column) {
                ghosts += event.ghosts;
            }
            assert!(ghosts > 0, "closed box cell {column},{row}");
        }
    }
}
