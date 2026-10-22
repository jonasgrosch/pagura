//! Hand-built scenes with analytic expectations. User space is device
//! space here (an identity request transform), on an RGB page.

use super::render;
use crate::request::{Rendering, Request};
use pagura_color::{ColorSpace, Components};
use pagura_core::{Limits, Position, Rect};
use pagura_interpret::{
    BlendMode, ClipId, ClipNode, Color, Composite, FillNode, GlyphSource, GlyphsNode, Group,
    GroupAttributes, GroupId, GroupRole, MarkInfo, MaskId, MaskKind, Node, NodeId, OutlineKey,
    PageSpace, Paint, PaintId, Placement, Scene, SoftMask, StateId, Tiling, TilingColors,
    TilingPaint,
};
use pagura_raster::{DeviceRect, FillRule, Path, Point, Rgba8, Transform, quantize_unit};
use std::sync::Arc;

struct Build {
    scene: Scene,
}

fn mark(state: StateId) -> MarkInfo {
    MarkInfo {
        state,
        clip: None,
        position: Position::Unknown,
    }
}

fn rect_path(x0: f64, y0: f64, x1: f64, y1: f64) -> Arc<Path> {
    Arc::new(Path::rect(Rect::from_corners(x0, y0, x1, y1).unwrap()))
}

impl Build {
    fn new(width: f64, height: f64) -> Self {
        let root = Group {
            role: GroupRole::Page,
            cause: pagura_interpret::GroupCause::Document,
            attributes: GroupAttributes::default(),
            bbox: None,
            view: None,
            children: Vec::new(),
            mark: mark(StateId(0)),
        };
        Self {
            scene: Scene {
                page: PageSpace {
                    crop: Rect::from_corners(0.0, 0.0, width, height).unwrap(),
                    rotation: pagura_document::Rotation::None,
                    user_unit: 1.0,
                },
                print_intent: false,
                root: GroupId(0),
                nodes: Vec::new(),
                groups: vec![root],
                masks: Vec::new(),
                tilings: Vec::new(),
                clips: Vec::new(),
                states: vec![Arc::new(Composite::default())],
                paints: Vec::new(),
                pens: Vec::new(),
                bytes: 0,
            },
        }
    }

    fn state(&mut self, edit: impl FnOnce(&mut Composite)) -> StateId {
        let mut state = Composite::default();
        edit(&mut state);
        self.scene.states.push(Arc::new(state));
        StateId(u32::try_from(self.scene.states.len() - 1).unwrap())
    }

    fn rgb(&mut self, rgb: [f64; 3]) -> PaintId {
        self.paint(Paint::Solid(Color {
            space: Arc::new(ColorSpace::DeviceRgb),
            components: Components::from_slice(&rgb),
        }))
    }

    fn paint(&mut self, paint: Paint) -> PaintId {
        self.scene.paints.push(Arc::new(paint));
        PaintId(u32::try_from(self.scene.paints.len() - 1).unwrap())
    }

    fn node(&mut self, parent: GroupId, node: Node) -> NodeId {
        self.scene.nodes.push(node);
        let id = NodeId(u32::try_from(self.scene.nodes.len() - 1).unwrap());
        self.scene.groups[parent.index()].children.push(id);
        id
    }

    fn fill(&mut self, parent: GroupId, path: Arc<Path>, paint: PaintId, info: MarkInfo) {
        self.node(
            parent,
            Node::Fill(FillNode {
                path,
                rule: FillRule::NonZero,
                paint,
                mark: info,
            }),
        );
    }

    fn rect(&mut self, parent: GroupId, r: [f64; 4], paint: PaintId, state: StateId) {
        self.fill(
            parent,
            rect_path(r[0], r[1], r[2], r[3]),
            paint,
            mark(state),
        );
    }

    fn group(&mut self, role: GroupRole, attributes: GroupAttributes, state: StateId) -> GroupId {
        self.scene.groups.push(Group {
            role,
            cause: pagura_interpret::GroupCause::Document,
            attributes,
            bbox: None,
            view: None,
            children: Vec::new(),
            mark: mark(state),
        });
        GroupId(u32::try_from(self.scene.groups.len() - 1).unwrap())
    }

    fn render(&self, background: Rgba8) -> Rendering {
        let region = pagura_paint::RenderRequest::page(&self.scene.page, 1.0)
            .unwrap()
            .page;
        let mut request = Request::new(Transform::IDENTITY, region);
        request.background = background;
        render(&self.scene, &request, &Limits::default()).unwrap()
    }
}

fn assert_pixel(rendering: &Rendering, (x, y): (u32, u32), expected: [f64; 4]) {
    let actual = rendering.exact(x, y).unwrap();
    for (a, e) in actual.iter().zip(expected) {
        assert!(
            (a - e).abs() < 1e-12,
            "({x}, {y}): {actual:?} against {expected:?}"
        );
    }
}

/// Source-over of a straight colour with alpha onto opaque white.
fn over_white(color: [f64; 3], alpha: f64) -> [f64; 4] {
    let c = color.map(|c| alpha * c + (1.0 - alpha));
    [c[0], c[1], c[2], 1.0]
}

fn level(coverage: f64) -> f64 {
    f64::from(quantize_unit(coverage)) / 255.0
}

#[test]
fn an_opaque_rectangle_composites_its_quantized_coverage() {
    let mut b = Build::new(4.0, 3.0);
    let red = b.rgb([1.0, 0.0, 0.0]);
    b.rect(GroupId(0), [0.25, 0.5, 3.0, 3.0], red, StateId(0));
    let r = b.render(Rgba8::WHITE);
    assert_pixel(&r, (0, 0), over_white([1.0, 0.0, 0.0], level(0.375)));
    assert_pixel(&r, (1, 0), over_white([1.0, 0.0, 0.0], level(0.5)));
    assert_pixel(&r, (1, 1), [1.0, 0.0, 0.0, 1.0]);
    assert_pixel(&r, (3, 2), [1.0, 1.0, 1.0, 1.0]);
    assert_eq!(&r.to_rgba8()[4..8], &[255, 127, 127, 255]);
}

#[test]
fn translucent_marks_and_blend_modes_follow_clause_11() {
    let mut b = Build::new(2.0, 1.0);
    let blue = b.rgb([0.0, 0.0, 1.0]);
    let gray = b.rgb([0.5, 0.5, 0.5]);
    let half = b.state(|s| s.alpha = 0.5);
    let multiply = b.state(|s| {
        s.alpha = 0.5;
        s.blend = BlendMode::Multiply;
    });
    b.rect(GroupId(0), [0.0, 0.0, 2.0, 1.0], blue, half);
    b.rect(GroupId(0), [1.0, 0.0, 2.0, 1.0], gray, multiply);
    let r = b.render(Rgba8::WHITE);
    // Over transparency, then over the white background at output.
    assert_pixel(&r, (0, 0), [0.5, 0.5, 1.0, 1.0]);
    // On the page: backdrop blue at alpha 1/2; B = Cb · Cs where the
    // backdrop exists. C = (1 - as) Cb ab + as ((1 - ab) Cs + ab Cb Cs).
    let (ab, r#as) = (0.5, 0.5);
    let channel = |cb: f64| (1.0 - r#as) * cb * ab + r#as * ((1.0 - ab) * 0.5 + ab * cb * 0.5);
    let alpha = r#as + ab * (1.0 - r#as);
    let premultiplied = [channel(0.0), channel(0.0), channel(1.0)];
    let expected = premultiplied.map(|c| c + (1.0 - alpha));
    assert_pixel(&r, (1, 0), [expected[0], expected[1], expected[2], 1.0]);
}

#[test]
fn groups_take_constant_alpha_knockout_and_isolation() {
    // A group of opaque red at alpha 1/2 equals red at alpha 1/2.
    let mut b = Build::new(3.0, 1.0);
    let red = b.rgb([1.0, 0.0, 0.0]);
    let blue = b.rgb([0.0, 0.0, 1.0]);
    let half = b.state(|s| s.alpha = 0.5);
    let group = b.group(GroupRole::Transparency, GroupAttributes::default(), half);
    b.node(GroupId(0), Node::Group(group));
    b.rect(group, [0.0, 0.0, 1.0, 1.0], red, StateId(0));
    // A knockout group: blue at alpha 1/2 over red replaces the red.
    let knockout = b.group(
        GroupRole::Transparency,
        GroupAttributes {
            knockout: true,
            isolated: true,
            space: None,
        },
        StateId(0),
    );
    b.node(GroupId(0), Node::Group(knockout));
    b.rect(knockout, [1.0, 0.0, 3.0, 1.0], red, StateId(0));
    b.rect(knockout, [2.0, 0.0, 3.0, 1.0], blue, half);
    let r = b.render(Rgba8::WHITE);
    assert_pixel(&r, (0, 0), over_white([1.0, 0.0, 0.0], 0.5));
    assert_pixel(&r, (1, 0), [1.0, 0.0, 0.0, 1.0]);
    assert_pixel(&r, (2, 0), over_white([0.0, 0.0, 1.0], 0.5));
}

#[test]
fn a_normal_non_isolated_group_is_invisible() {
    // 11.4.4: a non-isolated, non-knockout group composited with Normal at
    // full opacity gives the same result as its content painted directly.
    let scene = |grouped: bool| {
        let mut b = Build::new(2.0, 2.0);
        let teal = b.rgb([0.0, 0.5, 0.5]);
        let pink = b.rgb([1.0, 0.25, 0.75]);
        let translucent = b.state(|s| s.alpha = 0.75);
        let screen = b.state(|s| {
            s.blend = BlendMode::Screen;
            s.alpha = 0.6;
        });
        b.rect(GroupId(0), [0.0, 0.0, 1.5, 2.0], teal, translucent);
        let parent = if grouped {
            let group = b.group(
                GroupRole::Transparency,
                GroupAttributes::default(),
                StateId(0),
            );
            b.node(GroupId(0), Node::Group(group));
            group
        } else {
            GroupId(0)
        };
        b.rect(parent, [0.5, 0.25, 2.0, 2.0], pink, screen);
        b.render(Rgba8::TRANSPARENT)
    };
    let (direct, grouped) = (scene(false), scene(true));
    for (a, b) in direct.values().iter().zip(grouped.values()) {
        for (a, b) in a.iter().zip(b) {
            assert!((a - b).abs() < 1e-12);
        }
    }
}

#[test]
fn soft_masks_use_luminosity_or_alpha() {
    let mut b = Build::new(4.0, 1.0);
    let white = b.rgb([1.0, 1.0, 1.0]);
    let red = b.rgb([1.0, 0.0, 0.0]);
    let half = b.state(|s| s.alpha = 0.5);
    // Luminosity of white over a black backdrop on the left half.
    let luminosity = b.group(
        GroupRole::SoftMask {
            kind: MaskKind::Luminosity,
        },
        GroupAttributes::default(),
        StateId(0),
    );
    b.rect(luminosity, [0.0, 0.0, 2.0, 1.0], white, StateId(0));
    // Group alpha of a half-transparent fill on the right half.
    let alpha = b.group(
        GroupRole::SoftMask {
            kind: MaskKind::Alpha,
        },
        GroupAttributes::default(),
        StateId(0),
    );
    b.rect(alpha, [2.0, 0.0, 4.0, 1.0], white, half);
    for (group, kind) in [(luminosity, MaskKind::Luminosity), (alpha, MaskKind::Alpha)] {
        b.scene.masks.push(SoftMask {
            group,
            kind,
            backdrop: None,
            transfer: None,
            position: Position::Unknown,
        });
    }
    let masked = |mask: u32| move |s: &mut Composite| s.soft_mask = Some(MaskId(mask));
    let by_luminosity = b.state(masked(0));
    let by_alpha = b.state(masked(1));
    b.rect(GroupId(0), [0.0, 0.0, 4.0, 1.0], red, by_luminosity);
    b.rect(GroupId(0), [0.0, 0.0, 4.0, 1.0], red, by_alpha);
    let r = b.render(Rgba8::WHITE);
    assert_pixel(&r, (1, 0), [1.0, 0.0, 0.0, 1.0]);
    assert_pixel(&r, (3, 0), over_white([1.0, 0.0, 0.0], 0.5));
}

#[test]
fn clips_intersect_with_the_mark() {
    let mut b = Build::new(3.0, 3.0);
    let black = b.rgb([0.0, 0.0, 0.0]);
    b.scene.clips.push(ClipNode {
        parent: None,
        path: rect_path(0.0, 0.0, 1.5, 3.0),
        rule: FillRule::NonZero,
    });
    b.scene.clips.push(ClipNode {
        parent: Some(ClipId(0)),
        path: rect_path(0.0, 0.75, 3.0, 3.0),
        rule: FillRule::NonZero,
    });
    b.fill(
        GroupId(0),
        rect_path(0.0, 0.0, 3.0, 3.0),
        black,
        MarkInfo {
            clip: Some(ClipId(1)),
            ..mark(StateId(0))
        },
    );
    let r = b.render(Rgba8::WHITE);
    let gray = |coverage: f64| {
        let v = 1.0 - level(coverage);
        [v, v, v, 1.0]
    };
    assert_pixel(&r, (1, 0), gray(0.125));
    assert_pixel(&r, (0, 1), gray(1.0));
    assert_pixel(&r, (2, 2), gray(0.0));
}

#[test]
fn glyph_placements_are_separate_marks_at_exact_origins() {
    let mut b = Build::new(4.0, 2.0);
    let black = b.rgb([0.0, 0.0, 0.0]);
    let half = b.state(|s| s.alpha = 0.5);
    let square = rect_path(0.0, 0.0, 1.0, 1.0);
    b.node(
        GroupId(0),
        Node::Glyphs(GlyphsNode {
            sources: vec![GlyphSource {
                outline: square,
                key: OutlineKey {
                    font: pagura_interpret::FontKey(1),
                    glyph: 7,
                },
                linear: Transform::IDENTITY,
            }],
            placements: vec![
                Placement {
                    source: 0,
                    origin: Point::new(0.5, 0.0),
                },
                Placement {
                    source: 0,
                    origin: Point::new(1.0, 0.0),
                },
            ],
            positions: Vec::new(),
            paint: black,
            mark: mark(half),
        }),
    );
    let r = b.render(Rgba8::WHITE);
    // Pixel 1 is covered by both placements, half by the first: each
    // composites on its own, at alpha 1/2 times its coverage.
    let first = 0.5 * level(0.5);
    let second = 0.5;
    let v = (1.0 - first) * (1.0 - second);
    assert_pixel(&r, (1, 0), [v, v, v, 1.0]);
    assert_pixel(&r, (0, 1), [1.0, 1.0, 1.0, 1.0]);
}

#[test]
fn overlapping_quadratics_refine_ambiguous_coverage_bytes() {
    let mut b = Build::new(1.0, 1.0);
    let black = b.rgb([0.0, 0.0, 0.0]);
    let mut arch = pagura_raster::PathBuilder::new();
    // The quadratic control is (1/2, h), so its area above its chord is
    // h/3. A fixed fine polygon lies just below the half-level while the
    // original curve lies above it. Overlap exposes that byte flip in the
    // final composed value, even though each approximation is very fine.
    let area = 62.5 / 255.0 + 1e-6;
    let h = 3.0 * area;
    arch.move_to(Point::new(0.0, 0.0));
    arch.cubic_to(
        Point::new(1.0 / 3.0, 2.0 * h / 3.0),
        Point::new(2.0 / 3.0, 2.0 * h / 3.0),
        Point::new(1.0, 0.0),
    )
    .unwrap();
    arch.close();
    let arch = arch.finish();
    let pixel = DeviceRect {
        x0: 0,
        y0: 0,
        x1: 1,
        y1: 1,
    };
    let coarse = crate::exact::glyph(
        &arch,
        [1.0, 0.0, 0.0, 1.0],
        Point::default(),
        &Limits::default(),
    )
    .unwrap();
    let coarse_area = crate::area::areas(std::slice::from_ref(&coarse), pixel).get(0, 0);
    let error: f64 = coarse.hulls.iter().map(|hull| hull.error(0, 0)).sum();
    assert!((coarse_area - area).abs() <= error);
    assert_eq!(quantize_unit(coarse_area), 62);
    assert_eq!(crate::exact::coverage(&[coarse], pixel).unwrap(), [63]);

    let sources = [rect_path(0.0, 0.0, 6.0 / 255.0, 1.0), Arc::new(arch)]
        .into_iter()
        .enumerate()
        .map(|(i, outline)| GlyphSource {
            outline,
            key: OutlineKey {
                font: pagura_interpret::FontKey(1),
                glyph: u32::try_from(i).unwrap(),
            },
            linear: Transform::IDENTITY,
        })
        .collect();
    b.node(
        GroupId(0),
        Node::Glyphs(GlyphsNode {
            sources,
            placements: vec![
                Placement {
                    source: 0,
                    origin: Point::default(),
                },
                Placement {
                    source: 1,
                    origin: Point::default(),
                },
            ],
            positions: Vec::new(),
            paint: black,
            mark: mark(StateId(0)),
        }),
    );
    let value = (1.0 - 6.0 / 255.0) * (1.0 - 63.0 / 255.0);
    assert_pixel(&b.render(Rgba8::WHITE), (0, 0), [value, value, value, 1.0]);
}

#[test]
fn repeated_uncertified_quadratics_keep_the_true_coverage_byte() {
    let mut b = Build::new(1.0, 1.0);
    let black = b.rgb([0.0, 0.0, 0.0]);
    let mut path = pagura_raster::PathBuilder::new();
    // Coincident contours have one arch's nonzero covered set, but cannot
    // certify winding zero or one. Its true area h/3 is just above the
    // first half-level, where an inscribed polygon can select byte zero.
    let area = 0.500_001 / 255.0;
    let h = 3.0 * area;
    for _ in 0..32 {
        path.move_to(Point::default());
        path.cubic_to(
            Point::new(1.0 / 3.0, 2.0 * h / 3.0),
            Point::new(2.0 / 3.0, 2.0 * h / 3.0),
            Point::new(1.0, 0.0),
        )
        .unwrap();
        path.close();
    }
    let outline = Arc::new(path.finish());
    let limits = Limits::default();
    let geometry = pagura_raster::GlyphEdges::new(&outline, [1.0, 0.0, 0.0, 1.0], &limits).unwrap();
    assert!(!geometry.curved());
    b.node(
        GroupId(0),
        Node::Glyphs(GlyphsNode {
            sources: vec![GlyphSource {
                outline,
                key: OutlineKey {
                    font: pagura_interpret::FontKey(1),
                    glyph: 1,
                },
                linear: Transform::IDENTITY,
            }],
            placements: vec![
                Placement {
                    source: 0,
                    origin: Point::default(),
                };
                3
            ],
            positions: Vec::new(),
            paint: black,
            mark: mark(StateId(0)),
        }),
    );
    let expected = (1.0 - level(area)).powi(3);
    assert_pixel(
        &b.render(Rgba8::WHITE),
        (0, 0),
        [expected, expected, expected, 1.0],
    );
    let pixel = DeviceRect {
        x0: 0,
        y0: 0,
        x1: 1,
        y1: 1,
    };
    let request = pagura_paint::RenderRequest::new(Transform::IDENTITY, pixel, pixel);
    let store = pagura_core::Store::new(64 << 20);
    let prepared = pagura_paint::prepare(&b.scene, &request, &store, &limits).unwrap();
    let mut scratch = pagura_paint::Scratch::default();
    let mut actual = Vec::new();
    prepared
        .with_scratch(&mut scratch)
        .unwrap()
        .render(&mut actual);
    for &channel in &actual[..3] {
        assert!(
            (f64::from(channel) - 255.0 * expected).abs() <= 1.0,
            "composed output {actual:?} against analytic {}",
            255.0 * expected,
        );
    }
}

#[test]
fn tiling_instances_repeat_the_cell() {
    let mut b = Build::new(6.0, 1.0);
    let green = b.rgb([0.0, 1.0, 0.0]);
    let cell = b.group(GroupRole::Tiling, GroupAttributes::default(), StateId(0));
    b.rect(cell, [0.0, 0.0, 1.0, 1.0], green, StateId(0));
    b.scene.tilings.push(Tiling {
        cell,
        bbox: Rect::from_corners(0.0, 0.0, 2.0, 1.0).unwrap(),
        step: [2.0, 1.0],
        matrix: Transform::translate(0.5, 0.0),
        colors: TilingColors::Colored,
        spacing: pagura_interpret::TilingSpacing::NoDistortion,
    });
    let pattern = b.paint(Paint::Tiling(TilingPaint {
        pattern: pagura_interpret::PatternId(0),
        color: None,
    }));
    b.rect(GroupId(0), [0.0, 0.0, 6.0, 1.0], pattern, StateId(0));
    let r = b.render(Rgba8::WHITE);
    // Squares at 0.5..1.5, 2.5..3.5 and 4.5..5.5 cover half of every pixel.
    for x in 0..6 {
        let v = 1.0 - level(0.5);
        assert_pixel(&r, (x, 0), [v, 1.0, v, 1.0]);
    }
}

#[test]
fn the_background_is_premultiplied_once() {
    let mut b = Build::new(1.0, 1.0);
    let red = b.rgb([1.0, 0.0, 0.0]);
    let half = b.state(|s| s.alpha = 0.5);
    b.rect(GroupId(0), [0.0, 0.0, 1.0, 1.0], red, half);
    let r = b.render(Rgba8::new(0, 0, 255, 51));
    // Blue at alpha 51 premultiplies to 51; the red covers half of it.
    let back = 51.0 / 255.0;
    assert_pixel(&r, (0, 0), [0.5, 0.0, 0.5 * back, 0.5 + 0.5 * back]);
}

#[test]
fn malformed_scenes_are_errors_not_panics() {
    let mut b = Build::new(1.0, 1.0);
    b.rect(GroupId(0), [0.0, 0.0, 1.0, 1.0], PaintId(9), StateId(0));
    let region = DeviceRect {
        x0: 0,
        y0: 0,
        x1: 1,
        y1: 1,
    };
    assert!(
        render(
            &b.scene,
            &Request::new(Transform::IDENTITY, region),
            &Limits::default()
        )
        .is_err()
    );
    let group = b.group(GroupRole::Page, GroupAttributes::default(), StateId(0));
    b.scene.nodes.clear();
    b.scene.groups[0].children.clear();
    b.node(GroupId(0), Node::Group(group));
    assert!(
        render(
            &b.scene,
            &Request::new(Transform::IDENTITY, region),
            &Limits::default()
        )
        .is_err()
    );
}

#[test]
fn overprint_simulation_needs_overprinting_marks() {
    let simulation = |overprint: bool, nonzero: bool, simulate: bool| {
        let mut b = Build::new(1.0, 1.0);
        let cyan = b.paint(Paint::Solid(Color {
            space: Arc::new(ColorSpace::DeviceCmyk),
            components: Components::from_slice(&[1.0, 0.0, 0.0, 0.0]),
        }));
        let state = b.state(|s| {
            s.overprint.enabled = overprint;
            s.overprint.nonzero = nonzero;
        });
        b.rect(GroupId(0), [0.0, 0.0, 1.0, 1.0], cyan, state);
        crate::overprint::Simulation::discover(&b.scene, simulate, 64)
            .unwrap()
            .enabled
    };
    // OPM 1 with zero DeviceCMYK components needs a CMYK device.
    assert!(simulation(true, true, true));
    assert!(!simulation(true, false, true));
    assert!(!simulation(false, true, true));
    assert!(!simulation(true, true, false));
}

#[test]
fn page_content_is_clipped_to_the_crop_box_and_mask_sources_are_not() {
    // A crop box half a pixel short of the second column; the alpha mask's
    // source covers both columns.
    let mut b = Build::new(2.0, 1.0);
    b.scene.page.crop = Rect::from_corners(0.0, 0.0, 1.5, 1.0).unwrap();
    let black = b.rgb([0.0, 0.0, 0.0]);
    let mask = b.group(
        GroupRole::SoftMask {
            kind: MaskKind::Alpha,
        },
        GroupAttributes::default(),
        StateId(0),
    );
    b.rect(mask, [0.0, 0.0, 2.0, 1.0], black, StateId(0));
    b.scene.masks.push(SoftMask {
        group: mask,
        kind: MaskKind::Alpha,
        backdrop: None,
        transfer: None,
        position: Position::Unknown,
    });
    let masked = b.state(|s| s.soft_mask = Some(MaskId(0)));
    b.rect(GroupId(0), [0.0, 0.0, 2.0, 1.0], black, masked);
    let r = b.render(Rgba8::WHITE);
    assert_pixel(&r, (0, 0), [0.0, 0.0, 0.0, 1.0]);
    let v = 1.0 - level(0.5);
    assert_pixel(&r, (1, 0), [v, v, v, 1.0]);
}

#[test]
fn the_initial_pattern_colour_paints_nothing() {
    let mut b = Build::new(1.0, 1.0);
    let pattern = b.paint(Paint::Solid(Color {
        space: Arc::new(ColorSpace::Pattern(None)),
        components: Components::from_slice(&[]),
    }));
    b.rect(GroupId(0), [0.0, 0.0, 1.0, 1.0], pattern, StateId(0));
    assert_pixel(&b.render(Rgba8::WHITE), (0, 0), [1.0, 1.0, 1.0, 1.0]);
}

#[test]
fn exact_quantization_composes_unquantized_areas() {
    let mut b = Build::new(1.0, 1.0);
    let black = b.rgb([0.0, 0.0, 0.0]);
    b.rect(GroupId(0), [0.0, 0.0, 0.5, 0.501], black, StateId(0));
    let region = DeviceRect {
        x0: 0,
        y0: 0,
        x1: 1,
        y1: 1,
    };
    let mut request = Request::new(Transform::IDENTITY, region);
    let bytes = render(&b.scene, &request, &Limits::default()).unwrap();
    request.quantization = crate::request::Quantization::Exact;
    let exact = render(&b.scene, &request, &Limits::default()).unwrap();
    let area = 0.5 * 0.501;
    let v = 1.0 - level(area);
    assert_pixel(&bytes, (0, 0), [v, v, v, 1.0]);
    assert_pixel(&exact, (0, 0), [1.0 - area, 1.0 - area, 1.0 - area, 1.0]);
}
