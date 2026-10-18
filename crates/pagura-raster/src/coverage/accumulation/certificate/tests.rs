use super::super::scatter::Scatter;
use super::*;

#[test]
fn integer_orientation_rejects_crossings_overlaps_and_endpoint_touches() {
    assert!(intersects([0, 0], [8, 8], [0, 8], [8, 0]));
    assert!(intersects([0, 0], [8, 0], [4, 0], [12, 0]));
    assert!(intersects([0, 0], [8, 0], [8, 0], [8, 8]));
    assert!(!intersects([0, 0], [8, 0], [9, 0], [12, 0]));
    assert!(!intersects([0, 0], [8, 8], [0, 1], [7, 8]));
}

#[test]
fn perimeter_hits_choose_a_transverse_side_at_corners() {
    assert_eq!(hit([0, 0], 1, 0), Some((0, 1)));
    assert_eq!(hit([1 << 20, 0], -1, 0), Some((1 << 20, 1)));
    assert_eq!(hit([1 << 20, 1 << 20], 0, -1), Some((2 << 20, 1)));
    assert_eq!(hit([0, 1 << 20], 0, 1), Some((3 << 20, -1)));
    assert_eq!(hit([5, 7], 1, 1), None);
    assert_eq!(round(AREA / 2, AREA), 128);
    assert_eq!(round(AREA, AREA), 255);
}

fn band_cell(groups: usize, full_first: bool) -> (Geometry, Scatter) {
    use crate::coverage::Shape;
    use crate::{FillRule, PathBuilder, Point, Transform};
    use std::sync::Arc;

    let mut shapes = Vec::new();
    for group in 0..groups {
        let mut builder = PathBuilder::new();
        if group == 0 && full_first {
            // Two enclosing contours keep winding nonzero on both sides of
            // the oppositely oriented band, making this group constant-filled.
            for _ in 0..2 {
                builder.rect(-1.0, -1.0, 3.0, 3.0);
            }
        }
        builder.move_to(Point::new(-1.0, -1.0));
        for i in 0..=192 {
            let t = f64::from(i) / 192.0;
            builder.line_to(Point::new(t, t)).unwrap();
        }
        for (x, y) in [(2.0, 2.0), (2.03125, 2.0), (-0.96875, -1.0)] {
            builder.line_to(Point::new(x, y)).unwrap();
        }
        builder.close();
        shapes.push(Shape {
            geometry: crate::coverage::Geometry::Path(Arc::new(builder.finish())),
            transform: Transform::IDENTITY,
            rule: FillRule::NonZero,
        });
    }
    let limits = Limits::default();
    let mut work = 0;
    let mut geometry = Geometry::default();
    geometry.build(&shapes, &limits, &mut work).unwrap();
    let mut scatter = Scatter::default();
    scatter
        .prepare(&geometry, (0, 0), 1, &limits, &mut work)
        .unwrap();
    scatter
        .row(&geometry, (0, 0), 1, 0, &limits, &mut work)
        .unwrap();
    assert!(geometry.segments.len() <= 512 && scatter.pieces.len() <= 512);
    (geometry, scatter)
}

#[test]
fn each_group_gets_its_own_pair_cap_in_every_piece_order() {
    let (geometry, mut scatter) = band_cell(2, true);
    let mut certificates = Certificates::default();
    for order in 0..3 {
        if order == 1 {
            scatter.pieces.reverse();
        }
        if order == 2 {
            scatter.pieces.rotate_left(97);
        }
        let value = certificates
            .cell(
                &geometry,
                &scatter.pieces,
                &scatter.carry,
                [0, 0],
                &Limits::default(),
                &mut 0,
            )
            .unwrap();
        // The full first group intersects a strip between x=y and x=y+1/32.
        // Its exact cell area is 1/32-(1/32)^2/2 = 63/2048, rounding to 8.
        // Each group needs 192 cross-chain pairs, below its own cap of 256.
        assert_eq!(value, Some(8), "piece order {order}");
    }
}

#[test]
fn skipped_walk_pairs_consume_the_work_budget() {
    for groups in [1, 2] {
        let (geometry, scatter) = band_cell(groups, false);
        let mut chains = std::collections::BTreeMap::<_, usize>::new();
        for p in &scatter.pieces {
            if geometry.group(p.segment as usize) == 0 {
                *chains
                    .entry(geometry.segments[p.segment as usize].chain)
                    .or_default() += 1;
            }
        }
        let longest = chains.values().max().copied().unwrap();
        let skipped = longest * (longest - 1) / 2;
        assert!(skipped > 10_000);
        let limits = Limits {
            raster_work: skipped - 1,
            ..Limits::default()
        };
        let mut certificates = Certificates::default();
        let mut work = 0;
        let mut pairs = 0;
        let context = GroupCell {
            group: 0,
            prefix: scatter.carry[0],
            position: [0, 0],
        };
        assert_eq!(
            certificates
                .walk(
                    &geometry,
                    &scatter.pieces,
                    context,
                    &mut pairs,
                    &limits,
                    &mut work
                )
                .unwrap_err()
                .kind,
            pagura_core::ErrorKind::Limit
        );
        work = 0;
        pairs = 0;
        certificates
            .walk(
                &geometry,
                &scatter.pieces,
                context,
                &mut pairs,
                &Limits::default(),
                &mut work,
            )
            .unwrap();
        assert!(pairs > 0 && pairs <= 256);
        assert!(work >= skipped + scatter.pieces.len());
    }
}
