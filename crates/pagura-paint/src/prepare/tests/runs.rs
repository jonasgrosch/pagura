//! Glyph runs as one item render byte for byte as one item per placement,
//! whatever the format, group, mask, band height or region, and report the
//! same first error.

use super::assert_within_charges;
use crate::plan::ItemKind;
use crate::prepare::planner::runs::RUNS;
use crate::request::RenderRequest;
use crate::testing::{Build, check, index, mark, rect_path};
use crate::{Charges, Scratch};
use pagura_core::{Limits, Position, Store};
use pagura_interpret::{
    BlendMode, Composite, GlyphSource, GlyphsNode, GroupAttributes, GroupId, MarkInfo, MaskKind,
    Node, OutlineKey, Paint, PaintId, PatternId, Placement, StateId, Tiling, TilingColors,
    TilingPaint,
};
use pagura_raster::{DeviceRect, Point, Transform};

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
/// same charges at every band height, and that it has a run item with
/// placements rasterized together, or one without, as `gathers` says.
fn same_bytes(name: &str, b: &Build, gathers: bool) {
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
    let request = b.request(page, 32);
    let prepared = b
        .prepare(&request, &Limits::default())
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(
        prepared.plan.items.iter().any(|item| matches!(&item.kind,
                ItemKind::Run(run) if run.glyphs.iter().any(|g| g.alone) == gathers)),
        "{name}: no run item with placements alone: {gathers}"
    );
    for region in regions {
        let render = |runs, height| {
            let request = b.request(region, height);
            with_runs(runs, || render_in(b, &request, &Store::new(1 << 20)))
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

/// [`render`] with the band rasterizer and `store`.
fn render_in(b: &Build, request: &RenderRequest, store: &Store) -> (Vec<u8>, Charges) {
    render_with(b, request, store, &mut Scratch::default())
}

/// [`render_in`] in `scratch`.
fn render_with(
    b: &Build,
    request: &RenderRequest,
    store: &Store,
    scratch: &mut Scratch,
) -> (Vec<u8>, Charges) {
    let prepared = crate::prepare(&b.scene, request, store, &Limits::default())
        .unwrap_or_else(|e| panic!("{e}"));
    let mut rendering = prepared
        .with_scratch(scratch)
        .unwrap_or_else(|e| panic!("{e}"));
    let mut rows = Vec::new();
    rendering.render(&mut rows);
    assert_within_charges(&rendering, prepared.charges());
    (rows, prepared.charges())
}

fn page() -> (Build, [PaintId; 2]) {
    let mut b = Build::new(40.0, 24.0);
    let back = b.rgb([0.2, 0.5, 0.9]);
    let ink = b.rgb([0.9, 0.2, 0.1]);
    b.rect(PAGE, [1.5, 2.25, 37.0, 21.5], back, mark(StateId(0)));
    (b, [back, ink])
}

#[test]
fn opaque_runs_store_straight_into_n16() {
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
    same_bytes("opaque", &b, true);
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
        same_bytes(name, &b, true);
        check(&b);
    }
}

#[test]
fn runs_in_groups_masks_clips_and_patterns() {
    let b = groups();
    same_bytes("groups", &b, true);
    check(&b);
}

/// Runs in a knockout and an isolated group, a soft mask, a clip and a
/// tiling cell.
fn groups() -> Build {
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
    b
}

/// Runs, opaque, translucent and under a clip that keeps some placements
/// whole and cuts others, render as one item per placement does.
#[test]
fn clipped_runs_render_as_one_item_per_placement() {
    let (mut opaque, [_, ink]) = page();
    run(
        &mut opaque,
        PAGE,
        &line(14, [1.25, 3.5], 2.5),
        ink,
        mark(StateId(0)),
    );
    run(
        &mut opaque,
        PAGE,
        &line(9, [6.0, 12.75], 3.25),
        ink,
        mark(StateId(0)),
    );
    let half = opaque.state(|s| s.alpha = 0.5);
    run(
        &mut opaque,
        PAGE,
        &line(5, [30.0, 15.0], 3.0),
        ink,
        mark(half),
    );
    // A clip that keeps some placements whole and cuts others.
    let clip = opaque.clip(None, [3.25, 4.5, 30.5, 19.75]);
    let info = MarkInfo {
        clip: Some(clip),
        ..mark(StateId(0))
    };
    run(&mut opaque, PAGE, &line(12, [1.0, 15.0], 2.5), ink, info);
    same_bytes("clipped", &opaque, true);
}

/// Glyph edges kept in the store from an earlier render, with their hulls,
/// change no byte and no charge.
#[test]
fn kept_glyph_edges_change_nothing() {
    let (mut b, [_, ink]) = page();
    run(
        &mut b,
        PAGE,
        &line(14, [1.25, 3.5], 2.5),
        ink,
        mark(StateId(0)),
    );
    // Placements outside the region keep their edges unbuilt.
    run(
        &mut b,
        PAGE,
        &line(6, [2.0, 20.0], 2.5),
        ink,
        mark(StateId(0)),
    );
    let page = b.page();
    let tops = DeviceRect {
        y1: page.y1 / 2,
        ..page
    };
    let store = Store::new(1 << 20);
    for region in [tops, page] {
        let request = b.request(region, 16);
        let cold = render_in(&b, &request, &Store::new(1 << 20));
        let warm = render_in(&b, &request, &store);
        assert_eq!(cold, warm, "{region:?}");
    }
}

/// Scratch kept from an earlier render of the same area, whatever that
/// render drew, renders as new scratch.
#[test]
fn kept_scratch_renders_as_new_scratch() {
    let (mut b, [_, ink]) = page();
    run(
        &mut b,
        PAGE,
        &line(14, [1.25, 3.5], 2.5),
        ink,
        mark(StateId(0)),
    );
    let clip = b.clip(None, [3.25, 4.5, 30.5, 19.75]);
    let info = MarkInfo {
        clip: Some(clip),
        ..mark(StateId(0))
    };
    run(&mut b, PAGE, &line(12, [1.0, 15.0], 2.5), ink, info);
    let earlier = groups();
    let request = b.request(b.page(), 16);
    let new = render_in(&b, &request, &Store::new(1 << 20));
    let mut scratch = Scratch::default();
    render_with(&earlier, &request, &Store::new(1 << 20), &mut scratch);
    let kept = render_with(&b, &request, &Store::new(1 << 20), &mut scratch);
    assert_eq!(kept, new);
}

#[test]
fn stacked_long_and_scattered_runs() {
    let (mut b, [_, ink]) = page();
    let half = b.state(|s| s.alpha = 0.375);
    // Every placement on the same pixels: none rasterized together.
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
    same_bytes("stacked", &b, false);
    same_bytes("long", &b, true);
    check(&b);
}

/// A glyph whose device linear map is not finite, which preparation
/// refuses.
fn refused(glyph: u32) -> GlyphSource {
    GlyphSource {
        linear: Transform::new(f64::INFINITY, 0.0, 0.0, 1.0, 0.0, 0.0),
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
            sources: vec![source(3, [2.0, 3.0]), refused(9)],
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

#[test]
fn placement_byte_errors_are_the_same_with_cold_and_warm_glyph_edges() {
    let mut b = Build::new(16.0, 16.0);
    let ink = b.rgb([0.0, 0.0, 0.0]);
    let mut path = pagura_raster::PathBuilder::new();
    for _ in 0..10 {
        path.move_to(Point::new(1.0, 1.0));
        path.cubic_to(
            Point::new(1.0, 15.0),
            Point::new(15.0, 15.0),
            Point::new(12.0, 1.0),
        )
        .unwrap();
        path.close();
    }
    let source = GlyphSource {
        outline: std::sync::Arc::new(path.finish()),
        ..source(8, [1.0, 1.0])
    };
    b.node(
        PAGE,
        Node::Glyphs(GlyphsNode {
            sources: vec![source],
            placements: vec![Placement {
                source: 0,
                origin: Point::default(),
            }],
            positions: vec![(0, Position::Buffer(91))],
            paint: ink,
            mark: mark(StateId(0)),
        }),
    );
    let warm = Store::new(1 << 20);
    crate::prepare(&b.scene, &b.request(b.page(), 1), &warm, &Limits::default()).unwrap();
    let limits = Limits {
        render_bytes: 4096,
        ..Limits::default()
    };
    for height in HEIGHTS {
        let request = b.request(b.page(), height);
        let cold = crate::prepare(&b.scene, &request, &Store::new(1 << 20), &limits)
            .map(|_| ())
            .unwrap_err();
        let kept = crate::prepare(&b.scene, &request, &warm, &limits)
            .map(|_| ())
            .unwrap_err();
        assert_eq!(
            (cold.kind, cold.message, cold.position),
            (
                pagura_core::ErrorKind::Limit,
                "render memory budget exceeded",
                Position::Buffer(91)
            )
        );
        assert_eq!(
            (kept.kind, kept.message, kept.position),
            (cold.kind, cold.message, cold.position)
        );
    }
}
