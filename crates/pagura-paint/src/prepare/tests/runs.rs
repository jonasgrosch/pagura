//! Glyph runs as one item render byte for byte as one item per placement,
//! whatever the format, group, mask, band height or region, and report the
//! same first error.

use crate::plan::ItemKind;
use crate::prepare::planner::runs::RUNS;
use crate::request::{FastParams, RenderRequest};
use crate::testing::{Build, check, index, mark, rect_path};
use crate::{Charges, PreparedScene};
use pagura_core::{Limits, Position, Store};
use pagura_interpret::{
    BlendMode, Composite, GlyphSource, GlyphsNode, GroupAttributes, GroupId, MarkInfo, MaskKind,
    Node, OutlineKey, Paint, PaintId, PatternId, Placement, StateId, Tiling, TilingColors,
    TilingPaint,
};
use pagura_raster::{DeviceRect, PathBuilder, Pixmap, Point, Rgba8, Transform};
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
/// same charges at every band height, and that it has a run item with
/// placements rasterized together, or one without, as `gathers` says.
fn same_bytes(name: &str, b: &Build, fast: FastParams, gathers: bool) {
    same_bytes_with(name, b, fast, gathers, Backend::Rects);
}

/// The backend a test prepares with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Backend {
    /// The rectangle backend of the tests.
    Rects,
    /// The band rasterizer, whose binned glyphs are masks.
    Raster,
}

/// Prepares `request` with `backend`.
fn prepare(b: &Build, request: &RenderRequest, backend: Backend) -> PreparedScene {
    let prepared = match backend {
        Backend::Rects => b.prepare(request, &Limits::default()),
        Backend::Raster => {
            crate::prepare(&b.scene, request, &Store::new(1 << 20), &Limits::default())
        }
    };
    prepared.unwrap_or_else(|e| panic!("{e}"))
}

/// Renders `request` with `backend` into region-sized RGBA8 with the
/// reserved charges, checking that rendering used no more.
fn render(b: &Build, request: &RenderRequest, backend: Backend) -> (Vec<u8>, Charges) {
    if backend == Backend::Rects {
        return b.render_charged(request).unwrap_or_else(|e| panic!("{e}"));
    }
    let mut prepared = prepare(b, request, backend);
    let region = request.region;
    let mut target = Pixmap::new(
        region.width(),
        region.height(),
        Rgba8::TRANSPARENT,
        usize::MAX,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    prepared.render(&mut target);
    let (used, reserved) = (prepared.usage(), prepared.charges());
    assert!(
        used.render_bytes() <= reserved.render_bytes()
            && used.render_work() <= reserved.render_work()
            && used.function_work() <= reserved.function_work(),
        "used {used:?} beyond reserved {reserved:?}"
    );
    (target.data().to_vec(), reserved)
}

/// [`same_bytes`] with `backend`.
fn same_bytes_with(name: &str, b: &Build, fast: FastParams, gathers: bool, backend: Backend) {
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
    let prepared = prepare(b, &request, backend);
    assert!(
        prepared.plan().items.iter().any(|item| matches!(&item.kind,
                ItemKind::Run(run) if run.glyphs.iter().any(|g| g.alone) == gathers)),
        "{name}: no run item with placements alone: {gathers}"
    );
    for region in regions {
        let render = |runs, height| {
            let mut request = b.request(region, height);
            request.fast = fast;
            with_runs(runs, || render(b, &request, backend))
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
    same_bytes("opaque", &b, FastParams::EXACT, true);
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
    let b = groups();
    same_bytes("groups", &b, FastParams::EXACT, true);
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

/// Binned glyphs are masks: stored straight into the page under an opaque
/// store, through band coverage elsewhere, and as shapes where a clip cuts
/// them; runs of them render as one item per placement does.
#[test]
fn binned_masks_render_as_one_item_per_placement() {
    let bins = FastParams {
        glyph_origin_bins: true,
        ..FastParams::EXACT
    };
    let bytes = FastParams {
        byte_storage: true,
        ..bins
    };
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
    for fast in [bins, bytes, FastParams::COMPETITIVE] {
        same_bytes_with("binned opaque", &opaque, fast, true, Backend::Raster);
        same_bytes_with("binned groups", &groups(), fast, true, Backend::Raster);
    }
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
