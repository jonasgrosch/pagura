use super::*;
use crate::Point;
use pagura_core::{ErrorKind, Limits};

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

#[test]
fn listed_rows_are_admitted_beside_their_strip_list() {
    // Items crossing several strips: their rows stay charged while both
    // passes of the list read them, and are released before it returns.
    let lines: Vec<Line> = (0..48_u32)
        .map(|i| {
            let x = f64::from(i) * 0.25;
            Line {
                a: Point::new(x, 1.0),
                b: Point::new(x + 3.0, 21.0 + 2.0 * f64::from(i)),
            }
        })
        .collect();
    let bounds = DeviceRect {
        x0: 0,
        y0: 0,
        x1: 16,
        y1: 128,
    };
    let resolved = Resolved::default();
    let list = |max| {
        let mut bytes = Meter::new(max, RENDER_BYTES);
        listed((&lines, &[]), &resolved, bounds, 16, &mut bytes)
            .map(|(list, _)| (bytes.used(), list.retained_bytes()))
    };
    let (used, retained) = list(usize::MAX).unwrap();
    assert_eq!(used, retained);
    let peak = retained + lines.len() * size_of::<Option<(u32, u32)>>();
    assert_eq!(list(peak).unwrap(), (retained, retained));
    let error = list(peak - 1).unwrap_err();
    assert_eq!(error.message, RENDER_BYTES);
}

/// `stroke` of `path` in a 128 × 128 area under `limits`.
fn stroked(path: &Path, stroke: &Stroke, limits: &Limits) -> Result<Option<Shape>> {
    let options = ShapeOptions {
        area: Area {
            region: DeviceRect {
                x0: 0,
                y0: 0,
                x1: 128,
                y1: 128,
            },
            band_height: 16,
        },
        limits,
    };
    Shape::stroke(
        path,
        stroke,
        Transform::IDENTITY,
        Transform::IDENTITY,
        Adjustment::None,
        &options,
    )
}

/// Whether `stroke` of `path` renders in a 128 × 128 area under `limits`;
/// a refusal is a budget's.
fn renders(path: &Path, stroke: &Stroke, limits: &Limits) -> bool {
    match stroked(path, stroke, limits) {
        Ok(_) => true,
        Err(error) => {
            assert_eq!(error.kind, ErrorKind::Limit, "{error}");
            false
        }
    }
}

/// The strokes of the budget probes, each with a spine: twenty collinear
/// segments under a 12 px round pen (the reviewed case), a loop of short
/// segments, a mitred zigzag, and a fan dense enough for its rows to pass
/// over chunks.
fn probed() -> Vec<(Path, Stroke)> {
    let line = |points: &[(f64, f64)], closed: bool| {
        let mut path = crate::PathBuilder::new();
        for (i, &(x, y)) in points.iter().enumerate() {
            if i == 0 {
                path.move_to(Point::new(x, y));
            } else {
                path.line_to(Point::new(x, y)).unwrap();
            }
        }
        if closed {
            path.close();
        }
        path.finish()
    };
    let around = |count: u32, radius: f64| -> Vec<(f64, f64)> {
        (0..count)
            .map(|k| {
                let t = std::f64::consts::TAU * f64::from(k) / f64::from(count);
                (64.0 + radius * t.cos(), 64.0 + radius * t.sin())
            })
            .collect()
    };
    let collinear: Vec<_> = (0..=20)
        .map(|k| (20.0, 20.0 + 3.0 * f64::from(k)))
        .collect();
    let zigzag: Vec<_> = (0..24)
        .map(|k| (20.0 + 3.0 * f64::from(k), 40.0 + 2.0 * f64::from(k % 2)))
        .collect();
    let round = |width: f64| Stroke {
        width,
        cap: crate::LineCap::Round,
        join: crate::LineJoin::Round,
        ..Stroke::default()
    };
    let mitred = Stroke {
        width: 10.0,
        ..Stroke::default()
    };
    let probed = vec![
        (line(&collinear, false), round(12.0)),
        (line(&around(48, 6.0), true), round(12.0)),
        (line(&zigzag, false), mitred),
        (line(&around(64, 3.0), true), round(16.0)),
    ];
    for (path, stroke) in &probed {
        let (_, _, spine) = crate::stroke::polygon_pieces(
            path,
            Transform::IDENTITY,
            stroke,
            &Limits::default(),
            (Tiling::Unproven, crate::flatten::TOLERANCE),
            &mut Meter::new(usize::MAX, RENDER_BYTES),
        )
        .unwrap();
        assert!(spine.is_some(), "{stroke:?}");
    }
    probed
}

/// The least budget `renders` succeeds in, by halving from `most`: one it
/// succeeds in where the one below fails.
fn halved(renders: impl Fn(usize) -> bool, most: usize) -> usize {
    let (mut fails, mut least) = (0, most);
    assert!(renders(least));
    while least - fails > 1 {
        let middle = fails + (least - fails) / 2;
        if renders(middle) {
            least = middle;
        } else {
            fails = middle;
        }
    }
    least
}

/// Each of `strokes` renders, with spines, in every render budget it
/// renders in without, of those scanned: `near` bytes either side of the
/// least it renders in without spines, 8 apart, and `above` budgets spread
/// up to four times that. Success is not monotone in the budget, since
/// capacities double only where that fits, so halving finds a least budget
/// where the one below fails, and the budgets around it are scanned. Nor
/// does the shape keep more for the page's budget.
fn never_refused(strokes: &[(Path, Stroke)], (near, above): (usize, usize)) {
    for (path, stroke) in strokes {
        let bytes = |render_bytes| Limits {
            render_bytes,
            ..Limits::default()
        };
        let plain =
            |budget| crate::stroke::without_spines(|| renders(path, stroke, &bytes(budget)));
        let least = halved(plain, Limits::default().render_bytes);
        let around = (least.saturating_sub(near)..least + near).step_by(8);
        let spread = (least..4 * least).step_by((3 * least / above).max(8));
        for budget in around.chain(spread) {
            if plain(budget) {
                assert!(
                    renders(path, stroke, &bytes(budget)),
                    "{stroke:?} at {budget}"
                );
            }
        }
        let retained = || {
            stroked(path, stroke, &Limits::default())
                .unwrap()
                .map_or(0, |shape| shape.retained_bytes())
        };
        let plain = crate::stroke::without_spines(retained);
        assert!(retained() <= plain, "{stroke:?}: {} > {plain}", retained());
    }
}

/// Each of `strokes` renders, with spines, in every `raster_work` it renders
/// in without, of those scanned: `near` units either side of the least it
/// renders in without spines, and `above` budgets spread up to twice that.
/// Work is charged whatever the budget left, so halving finds the least.
/// Near it, the spine's own work, of the same size, runs out within regions.
fn never_raised(strokes: &[(Path, Stroke)], (near, above): (usize, usize)) {
    for (path, stroke) in strokes {
        let work = |raster_work| Limits {
            raster_work,
            ..Limits::default()
        };
        let plain = |budget| crate::stroke::without_spines(|| renders(path, stroke, &work(budget)));
        let least = halved(plain, Limits::default().raster_work);
        let around = least.saturating_sub(near)..least + near;
        let spread = (least..2 * least).step_by(least / above + 1);
        for budget in around.chain(spread) {
            if plain(budget) {
                assert!(
                    renders(path, stroke, &work(budget)),
                    "{stroke:?} at {budget}"
                );
            }
        }
    }
}

/// A spine never turns a render into a refusal for want of bytes: the
/// reviewed case and a dense fan, scanned near their least budgets.
#[test]
fn spines_never_turn_a_render_into_a_refusal() {
    let probed = probed();
    never_refused(&[probed[0].clone(), probed[3].clone()], (64, 8));
}

/// Nor for want of work: the shape's meter is charged no more with spines
/// than without, for the reviewed case and a dense fan.
#[test]
fn spines_never_raise_the_least_raster_work() {
    let probed = probed();
    never_raised(&[probed[0].clone(), probed[3].clone()], (8, 4));
}

/// Both, for every probed stroke, scanned wider: 8 bytes apart within a KiB
/// of the least budget and 128 budgets above, and 64 work units either side
/// and 64 above.
#[test]
#[ignore = "slow unoptimized; run with --release --ignored"]
fn spines_never_refuse_any_probed_budget() {
    let probed = probed();
    never_refused(&probed, (1024, 128));
    never_raised(&probed, (64, 64));
}
