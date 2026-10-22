//! Glyph runs as one item render byte for byte as one item per placement,
//! whatever the format, group, mask, band height or region, and report the
//! same first error.

use crate::plan::ItemKind;
use crate::prepare::planner::runs::RUNS;
use crate::request::FastParams;
use crate::testing::{Build, check, index, mark, rect_path};
use pagura_core::{Limits, Position};
use pagura_interpret::{
    BlendMode, Composite, GlyphSource, GlyphsNode, GroupAttributes, GroupId, MarkInfo, MaskKind,
    Node, OutlineKey, Paint, PaintId, PatternId, Placement, StateId, Tiling, TilingColors,
    TilingPaint,
};
use pagura_raster::{DeviceRect, PathBuilder, Point, Transform};
use std::sync::Arc;

const PAGE: GroupId = GroupId(0);
const HEIGHTS: [u32; 5] = [1, 5, 16, 32, 256];

fn source(glyph: u32, [w, h]: [f64; 2]) -> GlyphSource {
    GlyphSource {
        outline: rect_path([0.0, 0.0, w, h]),
        key: OutlineKey {
            font: pagura_interpret::FontKey(1),
            glyph,
        },
        linear: Transform::IDENTITY,
    }
}

/// A run of `origins` in `parent`, alternating two glyph shapes.
fn run(b: &mut Build, parent: GroupId, origins: &[[f64; 2]], paint: PaintId, info: MarkInfo) {
    let placements = origins
        .iter()
        .enumerate()
        .map(|(i, &[x, y])| Placement {
            source: u32::from(i % 3 == 2),
            origin: Point::new(x, y),
        })
        .collect();
    b.node(
        parent,
        Node::Glyphs(GlyphsNode {
            sources: vec![source(3, [2.75, 3.5]), source(4, [1.5, 2.25])],
            placements,
            positions: Vec::new(),
            paint,
            mark: info,
        }),
    );
}

/// A line of `count` placements `pitch` apart from `(x, y)`, some closer
/// than a glyph's width.
fn line(count: u32, [x, y]: [f64; 2], pitch: f64) -> Vec<[f64; 2]> {
    (0..count)
        .map(|i| {
            let i = f64::from(i);
            [x + i * pitch + 0.375 * (i % 3.0), y + 0.25 * (i % 2.0)]
        })
        .collect()
}

fn with_runs<T>(runs: bool, f: impl FnOnce() -> T) -> T {
    let before = RUNS.with(|r| r.replace(runs));
    let value = f();
    RUNS.with(|r| r.set(before));
    value
}

/// Asserts that `b` renders the same bytes with run items as with one
/// item per placement for every band height and a few regions, with the
/// same charges at every band height, and that it has a run item that
/// gathers or not as `gathers` says.
fn same_bytes(name: &str, b: &Build, fast: FastParams, gathers: bool) {
    let page = b.page();
    let half = page.x1 / 2;
    let regions = [
        page,
        DeviceRect {
            x0: 0,
            y0: 1,
            x1: half + 1,
            y1: page.y1,
        },
        DeviceRect {
            x0: half - 2,
            y0: 0,
            x1: page.x1,
            y1: page.y1 - 3,
        },
    ];
    let mut request = b.request(page, 32);
    request.fast = fast;
    let prepared = b
        .prepare(&request, &Limits::default())
        .unwrap_or_else(|e| panic!("{name}: {e}"));
    assert!(
        prepared
            .plan()
            .items
            .iter()
            .any(|item| matches!(&item.kind, ItemKind::Run(run) if run.gather == gathers)),
        "{name}: no run item that gathers: {gathers}"
    );
    for region in regions {
        let render = |runs, height| {
            let mut request = b.request(region, height);
            request.fast = fast;
            with_runs(runs, || {
                b.render_charged(&request)
                    .unwrap_or_else(|e| panic!("{name}: {e}"))
            })
        };
        let (_, charges) = render(true, 32);
        for height in HEIGHTS {
            let (bytes, reserved) = render(true, height);
            let (expected, _) = render(false, height);
            assert!(
                bytes == expected,
                "{name}: region {region:?} band height {height}"
            );
            assert_eq!(reserved, charges, "{name}: charges at band height {height}");
        }
    }
}

fn page() -> (Build, [PaintId; 2]) {
    let mut b = Build::new(40.0, 24.0);
    let back = b.rgb([0.2, 0.5, 0.9]);
    let ink = b.rgb([0.9, 0.2, 0.1]);
    b.rect(PAGE, [1.5, 2.25, 37.0, 21.5], back, mark(StateId(0)));
    (b, [back, ink])
}

#[test]
fn opaque_runs_store_each_placement_straight_into_n16() {
    let (mut b, [_, ink]) = page();
    run(
        &mut b,
        PAGE,
        &line(14, [1.25, 3.5], 2.5),
        ink,
        mark(StateId(0)),
    );
    run(
        &mut b,
        PAGE,
        &line(9, [6.0, 12.75], 3.25),
        ink,
        mark(StateId(0)),
    );
    same_bytes("opaque", &b, FastParams::EXACT, false);
    check(&b);
}

/// An edit of a composite state.
type Style = fn(&mut Composite);

#[test]
fn translucent_and_blended_runs_gather_where_placements_do_not_meet() {
    let styles: [(&str, Style); 3] = [
        ("half", |s| s.alpha = 0.5),
        ("multiply", |s| {
            s.alpha = 0.75;
            s.blend = BlendMode::Multiply;
        }),
        ("luminosity", |s| s.blend = BlendMode::Luminosity),
    ];
    for (name, edit) in styles {
        let (mut b, [_, ink]) = page();
        let state = b.state(edit);
        run(&mut b, PAGE, &line(14, [1.25, 3.5], 2.5), ink, mark(state));
        run(&mut b, PAGE, &line(5, [30.0, 15.0], 3.0), ink, mark(state));
        same_bytes(name, &b, FastParams::EXACT, true);
        check(&b);
        let bytes = FastParams {
            byte_storage: true,
            ..FastParams::EXACT
        };
        same_bytes(name, &b, bytes, true);
    }
}

#[test]
fn runs_in_groups_masks_clips_and_patterns() {
    let (mut b, [back, ink]) = page();
    let half = b.state(|s| s.alpha = 0.5);
    // A non-isolated knockout group of translucent runs.
    let knockout = b.transparency(
        PAGE,
        GroupAttributes {
            isolated: false,
            knockout: true,
            space: None,
        },
        mark(StateId(0)),
    );
    run(
        &mut b,
        knockout,
        &line(10, [2.0, 2.0], 2.25),
        ink,
        mark(half),
    );
    // An isolated translucent group.
    let isolated = b.transparency(
        PAGE,
        GroupAttributes {
            isolated: true,
            knockout: false,
            space: None,
        },
        mark(half),
    );
    run(
        &mut b,
        isolated,
        &line(8, [4.0, 8.0], 2.75),
        ink,
        mark(half),
    );
    // A soft mask, whose content is walked at its first use while the
    // run that uses it is being prepared, and a clip.
    let (mask, group) = b.mask(MaskKind::Luminosity, GroupAttributes::default(), None);
    b.rect(group, [0.0, 10.0, 25.0, 20.0], back, mark(StateId(0)));
    run(&mut b, group, &line(6, [8.0, 14.0], 2.0), ink, mark(half));
    let masked = b.state(|s| s.soft_mask = Some(mask));
    let clip = b.clip(None, [3.25, 11.5, 30.5, 19.75]);
    let info = MarkInfo {
        clip: Some(clip),
        ..mark(masked)
    };
    run(&mut b, PAGE, &line(12, [1.0, 13.0], 2.5), ink, info);
    // A run in a tiling cell.
    let cell = b.group(
        pagura_interpret::GroupRole::Tiling,
        GroupAttributes::default(),
        mark(StateId(0)),
    );
    b.scene.tilings.push(Tiling {
        cell,
        bbox: pagura_core::Rect::from_corners(0.0, 0.0, 6.0, 4.0).unwrap_or_else(|| unreachable!()),
        step: [6.0, 4.0],
        matrix: Transform::translate(0.5, 0.25),
        colors: TilingColors::Colored,
        spacing: pagura_interpret::TilingSpacing::NoDistortion,
    });
    run(&mut b, cell, &line(3, [0.25, 0.5], 1.75), ink, mark(half));
    let tiles = b.paint(Paint::Tiling(TilingPaint {
        pattern: PatternId(0),
        color: None,
    }));
    b.rect(PAGE, [24.25, 1.5, 39.0, 9.75], tiles, mark(StateId(0)));
    same_bytes("groups", &b, FastParams::EXACT, true);
    check(&b);
}

#[test]
fn stacked_long_and_scattered_runs() {
    let (mut b, [_, ink]) = page();
    let half = b.state(|s| s.alpha = 0.375);
    // Every placement on the same pixels: too many meetings to gather.
    run(&mut b, PAGE, &[[5.25, 6.5]; 12], ink, mark(half));
    // More placements than one run item holds.
    let long: Vec<[f64; 2]> = (0..300_u32)
        .map(|i| {
            let i = f64::from(i);
            [1.0 + (i * 1.3) % 35.0, 9.0 + (i / 27.0).floor() * 0.5]
        })
        .collect();
    run(&mut b, PAGE, &long, ink, mark(half));
    // Placements far apart: separate run items.
    run(
        &mut b,
        PAGE,
        &[
            [1.0, 1.0],
            [35.0, 19.0],
            [2.0, 19.5],
            [36.0, 1.5],
            [18.0, 10.0],
        ],
        ink,
        mark(half),
    );
    same_bytes("stacked", &b, FastParams::EXACT, false);
    same_bytes("long", &b, FastParams::EXACT, true);
    check(&b);
}

/// A glyph outline the rectangle backend refuses.
fn curved(glyph: u32) -> GlyphSource {
    let mut path = PathBuilder::new();
    path.move_to(Point::new(0.0, 0.0));
    let _ = path.cubic_to(
        Point::new(1.0, 0.0),
        Point::new(2.0, 1.0),
        Point::new(2.0, 2.0),
    );
    path.close();
    GlyphSource {
        outline: Arc::new(path.finish()),
        ..source(glyph, [1.0, 1.0])
    }
}

#[test]
fn the_first_refused_placement_reports_its_position() {
    let (mut b, [_, ink]) = page();
    let half = b.state(|s| s.alpha = 0.5);
    let placements = (0..8_u32)
        .map(|i| Placement {
            source: u32::from(i == 5),
            origin: Point::new(2.0 + 3.0 * f64::from(i), 6.0),
        })
        .collect();
    b.node(
        PAGE,
        Node::Glyphs(GlyphsNode {
            sources: vec![source(3, [2.0, 3.0]), curved(9)],
            placements,
            positions: (0..8)
                .map(|i| (index(i), Position::Buffer(100 + i)))
                .collect(),
            paint: ink,
            mark: mark(half),
        }),
    );
    for runs in [true, false] {
        let error = with_runs(runs, || {
            b.prepare(&b.request(b.page(), 16), &Limits::default())
                .map(|_| ())
                .unwrap_err()
        });
        assert_eq!(error.position, Position::Buffer(105), "runs {runs}");
    }
}
