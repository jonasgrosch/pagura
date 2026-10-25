use super::*;
use crate::Point;
use pagura_core::Limits;

#[test]
fn retained_union_rows_charge_before_allocating() {
    let line = Line {
        a: Point::new(0.0, 0.0),
        b: Point::new(1.0, 1.0),
    };
    let mut resolved = Resolved::default();
    let mut refused = Meter::new(0, RENDER_BYTES);
    assert!(resolved.push(0, &[line], &mut refused).is_err());
    assert_eq!(
        (resolved.lines.capacity(), resolved.rows.capacity()),
        (0, 0)
    );
    let mut bytes = Meter::new(size_of::<Line>() + size_of::<(u32, u32)>(), RENDER_BYTES);
    resolved.push(0, &[line], &mut bytes).unwrap();
    assert_eq!(bytes.used(), resolved.retained_bytes());
    assert!(resolved.push(1, &[line], &mut bytes).is_err());
    assert_eq!((resolved.lines.len(), resolved.rows.len()), (1, 1));
}

#[test]
fn empty_resolved_rows_also_need_storage() {
    let mut resolved = Resolved::default();
    assert!(
        resolved
            .push(0, &[], &mut Meter::new(0, RENDER_BYTES))
            .is_err()
    );
    assert_eq!(resolved.rows.capacity(), 0);
}

#[test]
fn an_empty_resolved_row_still_masks_the_original_polygon() {
    let limits = Limits::default();
    let mut path = crate::PathBuilder::new();
    path.move_to(Point::default());
    path.line_to(Point::new(0.0, 2.0)).unwrap();
    path.line_to(Point::new(1.0, 2.0)).unwrap();
    path.line_to(Point::new(1.0, 0.0)).unwrap();
    path.close();
    let mut bytes = crate::testing::bytes();
    let mut polygon = Polygon::from_lines(&path.finish(), Transform::IDENTITY, &limits, &mut bytes)
        .unwrap()
        .unwrap();
    polygon
        .cell(Point::new(0.25, 0.25), &limits, &mut bytes)
        .unwrap();
    let area = super::super::tests::area(0, 0, 2, 3, 2);
    let options = ShapeOptions {
        area,
        limits: &limits,
    };
    let shape = Shape::polygon(
        polygon,
        FillRule::NonZero,
        Winding::Unknown,
        &Prep::new(&options, None),
        &mut bytes,
        None,
    )
    .unwrap()
    .unwrap();
    assert_eq!(shape.resolved.mask(&(0..1)), 1);
    assert_eq!(shape.resolved.lines(&(0..1)), []);
    let (pixels, work) = crate::testing::raster(&area, Some(&shape), None);
    assert_eq!(pixels, [0, 0, 255, 0, 0, 0]);
    assert!(work <= shape.work());
}

fn round_capped(points: &[(f64, f64)]) -> Shape {
    let mut path = crate::PathBuilder::new();
    for (i, &(x, y)) in points.iter().enumerate() {
        if i == 0 {
            path.move_to(Point::new(x, y));
        } else {
            path.line_to(Point::new(x, y)).unwrap();
        }
    }
    let path = path.finish();
    let stroke = Stroke {
        width: 4.0,
        cap: crate::LineCap::Round,
        ..Stroke::default()
    };
    let limits = Limits::default();
    let options = ShapeOptions {
        area: Area {
            region: DeviceRect {
                x0: 0,
                y0: 0,
                x1: 16,
                y1: 128,
            },
            band_height: 32,
        },
        limits: &limits,
    };
    Shape::stroke(
        &path,
        &stroke,
        Transform::IDENTITY,
        Transform::IDENTITY,
        Adjustment::None,
        &options,
    )
    .unwrap()
    .unwrap()
}

#[test]
fn round_cap_overlap_resolves_only_uncertified_rows() {
    // An end segment shorter than the pen radius cannot hold its cap, so
    // the run keeps the canonical full circles, which overlap the body near
    // its ends. Their uncertain rows still resolve, but the long middle is
    // signed area.
    let shape = round_capped(&[(8.0, 5.0), (8.0, 55.0), (8.0, 105.0), (8.0, 106.5)]);
    assert_ne!(shape.resolved.rows, []);
    assert!(shape.resolved.rows.len() < shape.bounds.height() as usize / 4);
    assert!(
        shape
            .resolved
            .rows
            .iter()
            .all(|&(y, _)| !(9..=100).contains(&y))
    );
}

#[test]
fn a_lone_round_capped_segment_is_one_convex_piece() {
    let shape = round_capped(&[(8.0, 5.0), (8.0, 105.0)]);
    assert_eq!(shape.resolved.rows, []);
}

#[test]
fn end_segments_holding_their_round_caps_resolve_nothing() {
    for points in [
        [(8.0, 5.0), (8.0, 55.0), (8.0, 105.0)],
        [(6.0, 5.0), (10.0, 55.0), (6.0, 105.0)],
    ] {
        let shape = round_capped(&points);
        assert_eq!(shape.resolved.rows, [], "{points:?}");
    }
}

#[test]
fn a_glyph_point_cell_resolves_only_its_overlap_row() {
    let mut path = crate::PathBuilder::new();
    path.rect(2.0, 2.0, 6.0, 28.0);
    path.move_to(Point::new(4.25, 15.75));
    path.close();
    let limits = Limits::default();
    let edges = std::sync::Arc::new(
        crate::GlyphEdges::new(&path.finish(), [1.0, 0.0, 0.0, 1.0], &limits).unwrap(),
    );
    assert!(!edges.curved());
    let area = super::super::tests::area(0, 0, 16, 36, 4);
    let options = ShapeOptions {
        area,
        limits: &limits,
    };
    let shape = Shape::glyph(&edges, Point::default(), &options)
        .unwrap()
        .unwrap();
    assert_eq!(
        shape
            .resolved
            .rows
            .iter()
            .map(|&(y, _)| y)
            .collect::<Vec<_>>(),
        [15]
    );
    let (pixels, work) = crate::testing::raster(&area, Some(&shape), None);
    assert!(work <= shape.work());
    for y in 0..36 {
        for x in 0..16 {
            assert_eq!(
                pixels[y * 16 + x],
                if (2..8).contains(&x) && (2..30).contains(&y) {
                    255
                } else {
                    0
                }
            );
        }
    }
}

#[test]
fn a_work_limited_point_cell_sweep_keeps_full_resolution() {
    let limits = Limits::default();
    let mut path = crate::PathBuilder::new();
    for i in 0..128 {
        let offset = f64::from(i) / 256.0;
        path.rect(3.0 * f64::from(i) + 2.0, offset, 1.0, 32.0 - 2.0 * offset);
    }
    let mut bytes = crate::testing::bytes();
    let mut polygon = Polygon::from_lines(&path.finish(), Transform::IDENTITY, &limits, &mut bytes)
        .unwrap()
        .unwrap();
    // Disjoint rectangles with many distinct end heights exhaust the
    // bounded sweep even before the point cell is added.
    assert!(uncertified(&polygon, 0.0, &mut bytes).unwrap().is_none());
    polygon
        .cell(Point::new(388.3, 1.2), &limits, &mut bytes)
        .unwrap();
    assert!(uncertified(&polygon, 0.0, &mut bytes).unwrap().is_none());
    let area = super::super::tests::area(0, 0, 400, 36, 4);
    let options = ShapeOptions {
        area,
        limits: &limits,
    };
    let shape = Shape::polygon(
        polygon,
        FillRule::NonZero,
        Winding::Check(0.0),
        &Prep::new(&options, None),
        &mut bytes,
        None,
    )
    .unwrap()
    .unwrap();
    assert_eq!(shape.resolved.rows.len(), shape.bounds.height() as usize);
    let (_, work) = crate::testing::raster(&area, Some(&shape), None);
    assert!(work <= shape.work());
}
