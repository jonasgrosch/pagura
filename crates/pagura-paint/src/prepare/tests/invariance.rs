//! Pixels, errors and charges do not depend on band height, and regions
//! assemble into the page byte for byte (ADR 0034 §9).

use crate::Completion;
use crate::testing::{Build, assert_within_one_level, mark};
use pagura_core::{Limits, Position};
use pagura_interpret::{
    BlendMode, ClipId, ClipNode, FillNode, GroupAttributes, GroupId, MarkInfo, MaskKind, Node,
    Paint, PatternId, Pen, PenId, StateId, StrokeNode, Tiling, TilingColors, TilingPaint,
};
use pagura_raster::{Adjustment, DeviceRect, FillRule, PathBuilder, Point, Stroke, Transform};
use std::sync::Arc;

const PAGE: GroupId = GroupId(0);
const HEIGHTS: [u32; 5] = [1, 5, 16, 32, 256];

#[test]
fn distant_line_interpolation_survives_repeated_composition() {
    let n = 549_755_813_888.0;
    let u = 1.0 / 8192.0;
    let c = 7679.0 / 8192.0;
    let cases = [
        // Area=(1-u/2)^2/(2+u/n)=0.4999389667063951, coverage127.
        // Two black stores reference16384/255=64.25098039215686; old63
        // violates the unrounded one-level contract.
        ([(-n, -n), (n, n + u), (-n, n + u)], 2, 64),
        // Area0.0019645858556032185 rounds to coverage1. A false empty
        // clip loses all three stores: old255 instead of reference252.0117.
        ([(-n + c, -n), (n + c, n + u), (n + c, -n)], 3, 252),
    ];
    for (points, repeats, level) in cases {
        let mut path = PathBuilder::new();
        path.move_to(Point::new(points[0].0, points[0].1));
        for (x, y) in &points[1..] {
            path.line_to(Point::new(*x, *y)).unwrap();
        }
        path.close();
        let path = Arc::new(path.finish());
        for clipped in [false, true] {
            let mut b = Build::new(1.0, 1.0);
            let black = b.rgb([0.0; 3]);
            if clipped {
                b.scene.clips.push(ClipNode {
                    parent: None,
                    path: Arc::clone(&path),
                    rule: FillRule::NonZero,
                });
            }
            for _ in 0..repeats {
                let info = MarkInfo {
                    clip: clipped.then_some(ClipId(0)),
                    ..mark(StateId(0))
                };
                if clipped {
                    b.rect(PAGE, [-2.0, -2.0, 2.0, 2.0], black, info);
                } else {
                    b.node(
                        PAGE,
                        Node::Fill(FillNode {
                            path: Arc::clone(&path),
                            rule: FillRule::NonZero,
                            paint: black,
                            mark: info,
                        }),
                    );
                }
            }
            for height in HEIGHTS {
                let actual = b.render(&b.request(b.page(), height)).unwrap();
                assert_eq!(
                    actual,
                    [level, level, level, 255],
                    "clipped={clipped}, height={height}, repeats={repeats}"
                );
            }
        }
    }
}

/// A page that exercises every buffer kind: translucent marks, blend
/// modes, a knockout group, a soft mask, a tiling pattern and clips.
fn busy() -> Build {
    let mut b = Build::new(37.0, 29.0);
    let colors = [
        b.rgb([0.9, 0.1, 0.1]),
        b.rgb([0.1, 0.8, 0.3]),
        b.rgb([0.2, 0.3, 0.9]),
        b.rgb([0.7, 0.7, 0.2]),
    ];
    let states = [
        StateId(0),
        b.state(|s| s.alpha = 0.5),
        b.state(|s| {
            s.alpha = 0.8;
            s.blend = BlendMode::Multiply;
        }),
        b.state(|s| s.blend = BlendMode::ColorDodge),
    ];
    let clip = b.clip(None, [3.25, 2.5, 33.5, 27.75]);
    for i in 0..24_u32 {
        let x = f64::from(i * 7 % 31) + 0.25 * f64::from(i % 4);
        let y = f64::from(i * 11 % 23) + 0.125 * f64::from(i % 8);
        let info = MarkInfo {
            clip: (i % 3 == 0).then_some(clip),
            ..mark(states[(i % 4) as usize])
        };
        b.rect(
            PAGE,
            [x, y, x + 6.5, y + 4.75],
            colors[(i % 4) as usize],
            info,
        );
    }
    let knockout = b.transparency(
        PAGE,
        GroupAttributes {
            isolated: false,
            knockout: true,
            space: None,
        },
        mark(states[1]),
    );
    b.rect(
        knockout,
        [10.5, 10.25, 20.0, 18.5],
        colors[0],
        mark(states[1]),
    );
    b.rect(
        knockout,
        [14.25, 12.5, 25.75, 21.0],
        colors[2],
        mark(states[2]),
    );
    let (mask, group) = b.mask(MaskKind::Luminosity, GroupAttributes::default(), None);
    b.rect(group, [2.0, 15.5, 30.0, 28.0], colors[3], mark(states[1]));
    let masked = b.state(|s| s.soft_mask = Some(mask));
    b.rect(PAGE, [1.5, 14.75, 35.25, 27.5], colors[2], mark(masked));
    let cell = b.group(
        pagura_interpret::GroupRole::Tiling,
        GroupAttributes::default(),
        mark(StateId(0)),
    );
    b.scene.tilings.push(Tiling {
        cell,
        bbox: pagura_core::Rect::from_corners(0.0, 0.0, 3.0, 3.0).unwrap_or_else(|| unreachable!()),
        step: [3.0, 3.0],
        matrix: Transform::translate(0.5, 0.25),
        colors: TilingColors::Colored,
        spacing: pagura_interpret::TilingSpacing::NoDistortion,
    });
    b.rect(cell, [0.25, 0.5, 2.0, 2.25], colors[1], mark(states[1]));
    let tiles = b.paint(Paint::Tiling(TilingPaint {
        pattern: PatternId(0),
        color: None,
    }));
    b.rect(PAGE, [20.25, 1.5, 36.0, 12.75], tiles, mark(states[1]));
    b
}

fn region(x0: u32, y0: u32, x1: u32, y1: u32) -> DeviceRect {
    DeviceRect { x0, y0, x1, y1 }
}

#[test]
fn band_heights_change_neither_pixels_nor_charges() {
    let b = busy();
    let page = b.page();
    let (expected, charges) = b
        .render_charged(&b.request(page, 32))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_within_one_level(&expected, &b.reference());
    for height in HEIGHTS {
        let (bytes, reserved) = b
            .render_charged(&b.request(page, height))
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(bytes, expected, "band height {height}");
        assert_eq!(reserved, charges, "band height {height}");
    }
}

#[test]
fn regions_assemble_into_the_page() {
    let b = busy();
    let page = b.page();
    let whole = b
        .render(&b.request(page, 32))
        .unwrap_or_else(|e| panic!("{e}"));
    let width = page.width() as usize;
    let mut seed = 5_u32;
    let mut next = |bound: u32| {
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        (seed >> 8) % bound
    };
    for _ in 0..6 {
        let cut_x = 1 + next(page.x1 - 1);
        let cut_y = 1 + next(page.y1 - 1);
        let tiles = [
            region(0, 0, cut_x, cut_y),
            region(cut_x, 0, page.x1, cut_y),
            region(0, cut_y, cut_x, page.y1),
            region(cut_x, cut_y, page.x1, page.y1),
        ];
        let mut assembled = vec![0_u8; whole.len()];
        for tile in tiles {
            let height = HEIGHTS[next(5) as usize];
            let bytes = b
                .render(&b.request(tile, height))
                .unwrap_or_else(|e| panic!("{e}"));
            let tile_width = tile.width() as usize;
            for (row, y) in (tile.y0..tile.y1).enumerate() {
                let start = (y as usize * width + tile.x0 as usize) * 4;
                let source = &bytes[row * tile_width * 4..(row + 1) * tile_width * 4];
                assembled[start..start + tile_width * 4].copy_from_slice(source);
            }
        }
        assert_eq!(assembled, whole, "cut at ({cut_x}, {cut_y})");
    }
}

/// A stroke preparation refuses, with a singular pen, at `Buffer(offset)`.
fn refused(b: &mut Build, at: [f64; 2], offset: usize) {
    let mut path = PathBuilder::new();
    path.move_to(Point::new(at[0], at[1]));
    let _ = path.line_to(Point::new(at[0] + 2.0, at[1] + 2.0));
    b.scene.pens.push(Arc::new(Pen {
        stroke: Stroke::default(),
        transform: Transform::new(0.0, 0.0, 0.0, 0.0, 0.0, 0.0),
        adjustment: Adjustment::None,
    }));
    let pen = PenId(u32::try_from(b.scene.pens.len() - 1).unwrap());
    let paint = b.rgb([0.0, 0.0, 0.0]);
    b.node(
        PAGE,
        Node::Stroke(StrokeNode {
            path: Arc::new(path.finish()),
            pen,
            paint,
            mark: MarkInfo {
                position: Position::Buffer(offset),
                ..mark(StateId(0))
            },
        }),
    );
}

#[test]
fn the_first_error_in_source_order_wins_whatever_the_bands() {
    let mut b = busy();
    refused(&mut b, [30.0, 20.0], 2);
    refused(&mut b, [2.0, 3.0], 1);
    let page = b.page();
    for height in HEIGHTS {
        let error = b
            .prepare(&b.request(page, height), &Limits::default())
            .map(|_| ())
            .unwrap_err();
        assert_eq!(error.position, Position::Buffer(2), "band height {height}");
    }
    // A region that the first refused mark does not meet reports the next.
    let error = b
        .prepare(&b.request(region(0, 0, 10, 10), 5), &Limits::default())
        .map(|_| ())
        .unwrap_err();
    assert_eq!(error.position, Position::Buffer(1));
    // And one that neither meets renders.
    assert!(
        b.prepare(&b.request(region(10, 0, 20, 10), 5), &Limits::default())
            .is_ok()
    );
}

#[test]
fn budgets_fail_in_source_order_and_bound_what_rendering_uses() {
    let b = busy();
    let page = b.page();
    let (_, charges) = b
        .render_charged(&b.request(page, 32))
        .unwrap_or_else(|e| panic!("{e}"));
    for (field, total) in [
        (0, charges.render_bytes()),
        (1, charges.render_work()),
        (2, charges.function_work()),
    ] {
        let limits = |max: usize| {
            let mut limits = Limits::default();
            match field {
                0 => limits.render_bytes = max,
                1 => limits.render_work = max,
                _ => limits.function_work = max,
            }
            limits
        };
        assert!(b.prepare(&b.request(page, 32), &limits(total)).is_ok());
        if total == 0 {
            continue;
        }
        for height in HEIGHTS {
            let error = b
                .prepare(&b.request(page, height), &limits(total - 1))
                .map(|_| ())
                .unwrap_err();
            assert_eq!(error.kind, pagura_core::ErrorKind::Limit);
        }
    }
}

#[test]
fn geometry_preparation_uses_what_earlier_marks_leave() {
    // Enough vertices make the polygon and its edge copy overlap by more
    // than the final render's scratch; each input separately still fits.
    let mut path = PathBuilder::new();
    path.polygon((0..8000_u32).map(|i| {
        let angle = f64::from(i) * std::f64::consts::TAU / 8000.0;
        Point::new(2.0 + 1.7 * angle.cos(), 2.0 + 1.7 * angle.sin())
    }));
    let path = Arc::new(path.finish());
    let geometry = |b: &mut Build| {
        let paint = b.rgb([0.0, 0.0, 0.0]);
        b.node(
            PAGE,
            Node::Fill(FillNode {
                path: Arc::clone(&path),
                rule: FillRule::NonZero,
                paint,
                mark: MarkInfo {
                    position: Position::Buffer(7),
                    ..mark(StateId(0))
                },
            }),
        );
    };
    let limits = Limits {
        render_bytes: 390 * 1024,
        ..Limits::default()
    };
    let mut alone = Build::new(4.0, 4.0);
    geometry(&mut alone);
    alone
        .prepare(&alone.request(alone.page(), 1), &limits)
        .unwrap();
    let mut occupied = Build::new(4.0, 4.0);
    let paint = occupied.rgb([0.0, 0.0, 0.0]);
    for _ in 0..40 {
        occupied.rect(PAGE, [0.25, 0.25, 3.75, 3.75], paint, mark(StateId(0)));
    }
    assert!(
        occupied
            .prepare(&occupied.request(occupied.page(), 1), &limits)
            .is_ok()
    );
    geometry(&mut occupied);
    refused(&mut occupied, [1.0, 1.0], 11);
    for height in HEIGHTS {
        let error = occupied
            .prepare(&occupied.request(occupied.page(), height), &limits)
            .map(|_| ())
            .unwrap_err();
        assert_eq!(
            (error.kind, error.message, error.position),
            (
                pagura_core::ErrorKind::Limit,
                "render memory budget exceeded",
                Position::Buffer(7)
            ),
            "band height {height}"
        );
    }
}

#[test]
fn prepared_scenes_can_be_shared_between_threads() {
    fn shared<T: Send + Sync>() {}
    shared::<crate::PreparedScene>();
}

#[test]
fn rendering_stops_between_bands() {
    let b = busy();
    let page = b.page();
    let prepared = b
        .prepare(&b.request(page, 8), &Limits::default())
        .unwrap_or_else(|e| panic!("{e}"));
    let mut target = Vec::new();
    let mut scratch = crate::Scratch::default();
    let mut rendering = prepared
        .with_scratch(&mut scratch)
        .unwrap_or_else(|e| panic!("{e}"));
    let mut bands = 0;
    let completion = rendering.render_until(&mut target, |row| {
        bands += 1;
        row >= 16
    });
    assert_eq!(completion, Completion::Stopped { row: 16 });
    assert_eq!(bands, 3);
    let whole = b
        .render(&b.request(page, 8))
        .unwrap_or_else(|e| panic!("{e}"));
    let rows = 16 * page.width() as usize * 4;
    assert_eq!(target.len(), rows);
    assert_eq!(target, whole[..rows]);
}

#[test]
fn reused_scratch_forgets_scenes_backgrounds_and_stopped_renders() {
    use pagura_raster::Rgba8;

    let wide = busy();
    let mut narrow = Build::new(37.0, 29.0);
    let red = narrow.rgb([0.8, 0.1, 0.2]);
    narrow.rect(PAGE, [2.25, 3.5, 28.75, 25.25], red, mark(StateId(0)));
    let mut changed = Build::new(37.0, 29.0);
    let green = changed.rgb([0.1, 0.7, 0.2]);
    changed.rect(PAGE, [6.25, 5.5, 32.75, 27.25], green, mark(StateId(0)));
    let mut scratch = crate::Scratch::default();
    for height in [5, 8] {
        for (scene, background) in [
            (&narrow, Rgba8::WHITE),
            (&changed, Rgba8::WHITE),
            (&narrow, Rgba8::new(20, 40, 60, 128)),
            (&changed, Rgba8::TRANSPARENT),
            (&wide, Rgba8::WHITE),
            (&wide, Rgba8::WHITE),
            (&narrow, Rgba8::WHITE),
        ] {
            let mut request = scene.request(scene.page(), height);
            request.background = background;
            let (expected, charges) = scene.render_charged(&request).unwrap();
            let prepared = scene.prepare(&request, &Limits::default()).unwrap();
            assert_eq!(prepared.charges(), charges);
            let mut stopped = Vec::new();
            let completion = prepared
                .with_scratch(&mut scratch)
                .unwrap()
                .render_until(&mut stopped, |row| row >= 16);
            assert!(matches!(completion, Completion::Stopped { .. }));
            assert_eq!(stopped, expected[..stopped.len()]);
            for _ in 0..2 {
                let mut rows = Vec::new();
                let mut rendering = prepared.with_scratch(&mut scratch).unwrap();
                rendering.render(&mut rows);
                super::assert_within_charges(&rendering, charges);
                assert_eq!(rows, expected);
            }
        }
    }
}
