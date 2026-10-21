//! Compositing semantics against the reference: every channel within one
//! level of its exact value.

use crate::testing::{Build, check, mark, rect_path};
use pagura_color::{ColorSpace, Process};
use pagura_interpret::{
    BlendMode, GlyphSource, GlyphsNode, GroupAttributes, GroupId, MarkInfo, MaskKind, Node,
    OutlineKey, Placement, StateId,
};
use pagura_raster::{Point, Rgba8, Transform};

const MODES: [BlendMode; 16] = [
    BlendMode::Normal,
    BlendMode::Multiply,
    BlendMode::Screen,
    BlendMode::Overlay,
    BlendMode::Darken,
    BlendMode::Lighten,
    BlendMode::ColorDodge,
    BlendMode::ColorBurn,
    BlendMode::HardLight,
    BlendMode::SoftLight,
    BlendMode::Difference,
    BlendMode::Exclusion,
    BlendMode::Hue,
    BlendMode::Saturation,
    BlendMode::Color,
    BlendMode::Luminosity,
];

const PAGE: GroupId = GroupId(0);

fn with_clip(state: StateId, clip: pagura_interpret::ClipId) -> MarkInfo {
    MarkInfo {
        clip: Some(clip),
        ..mark(state)
    }
}

#[test]
fn translucent_marks_and_clips() {
    let mut b = Build::new(9.0, 7.0);
    let red = b.rgb([1.0, 0.1, 0.0]);
    let blue = b.rgb([0.0, 0.3, 0.9]);
    let half = b.state(|s| s.alpha = 0.5);
    let third = b.state(|s| s.alpha = 0.3);
    let clip = b.clip(None, [1.5, 0.75, 8.25, 6.5]);
    let inner = b.clip(Some(clip), [0.0, 2.25, 6.75, 7.0]);
    b.rect(PAGE, [0.25, 0.5, 6.0, 4.75], red, mark(half));
    b.rect(PAGE, [2.5, 1.25, 8.5, 6.5], blue, with_clip(third, clip));
    b.rect(PAGE, [0.0, 0.0, 9.0, 7.0], red, with_clip(third, inner));
    check(&b);
}

#[test]
fn every_blend_mode_matches_the_reference() {
    for mode in MODES {
        let mut b = Build::new(6.0, 4.0);
        let back = b.rgb([0.2, 0.7, 0.4]);
        let front = b.rgb([0.9, 0.35, 0.6]);
        let translucent = b.state(|s| s.alpha = 0.6);
        let blended = b.state(|s| {
            s.alpha = 0.8;
            s.blend = mode;
        });
        b.rect(PAGE, [0.5, 0.25, 4.5, 3.75], back, mark(translucent));
        b.rect(PAGE, [2.25, 1.5, 6.0, 4.0], front, mark(blended));
        check(&b);
        // The same inside an isolated and a non-isolated group with alpha.
        for isolated in [false, true] {
            let mut b = Build::new(6.0, 4.0);
            let back = b.rgb([0.2, 0.7, 0.4]);
            let front = b.rgb([0.9, 0.35, 0.6]);
            let group_state = b.state(|s| s.alpha = 0.9);
            let blended = b.state(|s| {
                s.alpha = 0.8;
                s.blend = mode;
            });
            b.rect(PAGE, [0.5, 0.25, 4.5, 3.75], back, mark(StateId(0)));
            let attributes = GroupAttributes {
                isolated,
                ..GroupAttributes::default()
            };
            let group = b.transparency(PAGE, attributes, mark(group_state));
            b.rect(group, [2.25, 1.5, 6.0, 4.0], front, mark(blended));
            b.rect(group, [1.25, 0.5, 3.0, 2.5], back, mark(blended));
            check(&b);
        }
    }
}

#[test]
fn knockout_groups_replace_by_shape() {
    for isolated in [false, true] {
        let mut b = Build::new(8.0, 5.0);
        let back = b.rgb([0.1, 0.5, 0.9]);
        let a = b.rgb([0.9, 0.2, 0.1]);
        let c = b.rgb([0.3, 0.9, 0.2]);
        let half = b.state(|s| s.alpha = 0.5);
        let multiply = b.state(|s| {
            s.alpha = 0.7;
            s.blend = BlendMode::Multiply;
        });
        b.rect(PAGE, [0.0, 0.0, 8.0, 5.0], back, mark(half));
        let attributes = GroupAttributes {
            isolated,
            knockout: true,
            space: None,
        };
        let group = b.transparency(PAGE, attributes, mark(StateId(0)));
        b.rect(group, [0.5, 0.5, 5.25, 4.25], a, mark(half));
        b.rect(group, [2.75, 1.5, 7.5, 4.75], c, mark(multiply));
        b.rect(group, [3.25, 0.25, 4.0, 3.0], a, mark(half));
        check(&b);
    }
}

#[test]
fn opaque_knockout_members_composite_as_source_over() {
    let mut b = Build::new(6.0, 4.0);
    let a = b.rgb([0.9, 0.2, 0.1]);
    let c = b.rgb([0.3, 0.9, 0.2]);
    let attributes = GroupAttributes {
        isolated: false,
        knockout: true,
        space: None,
    };
    let group = b.transparency(PAGE, attributes, mark(StateId(0)));
    b.rect(group, [0.5, 0.5, 4.25, 3.25], a, mark(StateId(0)));
    b.rect(group, [2.75, 1.5, 6.0, 4.0], c, mark(StateId(0)));
    check(&b);
}

#[test]
fn soft_masks_by_alpha_and_luminosity() {
    for (kind, isolated, backdrop) in [
        (MaskKind::Alpha, true, None),
        (
            MaskKind::Luminosity,
            false,
            Some([0.25, 0.5, 0.75].as_slice()),
        ),
        (
            MaskKind::Luminosity,
            true,
            Some([0.25, 0.5, 0.75].as_slice()),
        ),
        (MaskKind::Luminosity, false, None),
    ] {
        let mut b = Build::new(8.0, 6.0);
        let gray = b.rgb([0.6, 0.6, 0.6]);
        let red = b.rgb([1.0, 0.0, 0.0]);
        let half = b.state(|s| s.alpha = 0.5);
        let attributes = GroupAttributes {
            isolated,
            ..GroupAttributes::default()
        };
        let (mask, group) = b.mask(kind, attributes, backdrop);
        b.rect(group, [1.0, 1.0, 6.5, 4.5], gray, mark(half));
        b.rect(group, [3.25, 2.25, 7.75, 5.75], red, mark(StateId(0)));
        let masked = b.state(|s| s.soft_mask = Some(mask));
        let blue = b.rgb([0.1, 0.2, 0.8]);
        b.rect(PAGE, [0.5, 0.5, 7.5, 5.5], blue, mark(masked));
        // A masked group as well.
        let masked_group = b.state(|s| {
            s.soft_mask = Some(mask);
            s.alpha = 0.75;
        });
        let inner = b.transparency(PAGE, GroupAttributes::default(), mark(masked_group));
        b.rect(inner, [0.0, 2.0, 8.0, 3.5], red, mark(StateId(0)));
        check(&b);
    }
}

#[test]
fn alpha_is_shape_reaches_knockout() {
    let mut b = Build::new(6.0, 3.0);
    let a = b.rgb([0.9, 0.2, 0.1]);
    let c = b.rgb([0.3, 0.9, 0.2]);
    let shaped = b.state(|s| {
        s.alpha = 0.4;
        s.alpha_is_shape = true;
    });
    let attributes = GroupAttributes {
        isolated: true,
        knockout: true,
        space: None,
    };
    let group = b.transparency(PAGE, attributes, mark(StateId(0)));
    b.rect(group, [0.0, 0.0, 4.5, 3.0], a, mark(shaped));
    b.rect(group, [2.25, 0.5, 6.0, 2.75], c, mark(shaped));
    check(&b);
}

/// The RGB of additive components in `process` (10.4.2).
fn to_rgb(process: Process, c: &[f64]) -> [f64; 3] {
    match process {
        Process::Gray => [c[0]; 3],
        Process::Rgb => [c[0], c[1], c[2]],
        Process::Cmyk => [0, 1, 2].map(|i| (c[i] + c[3] - 1.0).max(0.0)),
    }
}

/// Asserts pixel `(x, y)` of a page `width` wide within one level of
/// `expected` (premultiplied RGBA in `0..=1`).
fn assert_pixel(bytes: &[u8], width: usize, (x, y): (usize, usize), expected: [f64; 4]) {
    let start = (y * width + x) * 4;
    for (channel, (&byte, value)) in bytes[start..start + 4].iter().zip(expected).enumerate() {
        assert!(
            (f64::from(byte) - value * 255.0).abs() <= 1.0,
            "({x}, {y}) channel {channel}: {byte} against {}",
            value * 255.0
        );
    }
}

#[test]
fn group_blending_spaces_convert_their_results() {
    for (process, space, components, additive) in [
        (
            Process::Gray,
            ColorSpace::DeviceGray,
            vec![0.35],
            vec![0.35],
        ),
        (
            Process::Cmyk,
            ColorSpace::DeviceCmyk,
            vec![0.1, 0.6, 0.2, 0.15],
            vec![0.9, 0.4, 0.8, 0.85],
        ),
    ] {
        let mut b = Build::new(6.0, 4.0);
        let back = b.rgb([0.2, 0.7, 0.4]);
        let paint = b.color(space.clone(), &components);
        let half = b.state(|s| s.alpha = 0.55);
        b.rect(PAGE, [0.0, 0.0, 4.0, 4.0], back, mark(StateId(0)));
        let attributes = GroupAttributes {
            isolated: true,
            knockout: false,
            space: Some(process),
        };
        let group_state = b.state(|s| s.alpha = 0.8);
        let group = b.transparency(PAGE, attributes, mark(group_state));
        b.rect(group, [1.0, 0.0, 6.0, 4.0], paint, mark(half));
        let bytes = b.render_page();
        // Over the backdrop: (1 - a) b + a c with a = 0.55 · 0.8.
        let rgb = to_rgb(process, &additive);
        let a = 0.55 * 0.8;
        let back = [0.2, 0.7, 0.4];
        let over = [0, 1, 2].map(|i| (1.0 - a) * back[i] + a * rgb[i]);
        assert_pixel(&bytes, 6, (2, 1), [over[0], over[1], over[2], 1.0]);
        // Over the white background where there is no backdrop.
        let white = [0, 1, 2].map(|i| a * rgb[i] + (1.0 - a));
        assert_pixel(&bytes, 6, (5, 2), [white[0], white[1], white[2], 1.0]);
    }
}

#[test]
fn page_groups_in_gray_and_cmyk_convert_at_output() {
    for (process, space, components, additive) in [
        (
            Process::Gray,
            ColorSpace::DeviceGray,
            vec![0.35],
            vec![0.35],
        ),
        (
            Process::Cmyk,
            ColorSpace::DeviceCmyk,
            vec![0.1, 0.6, 0.2, 0.15],
            vec![0.9, 0.4, 0.8, 0.85],
        ),
    ] {
        let mut b = Build::new(5.0, 3.0);
        b.scene.groups[0].attributes.space = Some(process);
        let paint = b.color(space, &components);
        let half = b.state(|s| s.alpha = 0.5);
        b.rect(PAGE, [0.0, 0.0, 3.0, 3.0], paint, mark(StateId(0)));
        b.rect(PAGE, [2.0, 0.0, 5.0, 2.5], paint, mark(half));
        let bytes = b.render_page();
        let rgb = to_rgb(process, &additive);
        assert_pixel(&bytes, 5, (1, 1), [rgb[0], rgb[1], rgb[2], 1.0]);
        assert_pixel(&bytes, 5, (2, 1), [rgb[0], rgb[1], rgb[2], 1.0]);
        let half_white = rgb.map(|c| 0.5 * c + 0.5);
        assert_pixel(
            &bytes,
            5,
            (4, 0),
            [half_white[0], half_white[1], half_white[2], 1.0],
        );
        let quarter = rgb.map(|c| 0.25 * c + 0.75);
        assert_pixel(&bytes, 5, (4, 2), [quarter[0], quarter[1], quarter[2], 1.0]);
    }
}

#[test]
fn backgrounds_are_premultiplied_once() {
    for alpha in [0.0, 1.0, 0.5] {
        let mut b = Build::new(4.0, 2.0);
        b.background = Rgba8::new(200, 100, 50, 128);
        let red = b.rgb([1.0, 0.0, 0.0]);
        let translucent = b.state(|s| s.alpha = alpha);
        b.rect(PAGE, [0.5, 0.0, 3.25, 1.5], red, mark(translucent));
        check(&b);
    }
}

#[test]
fn glyph_placements_composite_separately() {
    let mut b = Build::new(8.0, 4.0);
    let red = b.rgb([1.0, 0.0, 0.0]);
    let half = b.state(|s| s.alpha = 0.5);
    let source = GlyphSource {
        outline: rect_path([0.0, 0.0, 3.0, 2.5]),
        key: OutlineKey {
            font: pagura_interpret::FontKey(1),
            glyph: 7,
        },
        linear: Transform::IDENTITY,
    };
    b.node(
        PAGE,
        Node::Glyphs(GlyphsNode {
            sources: vec![source],
            placements: vec![
                Placement {
                    source: 0,
                    origin: Point::new(0.5, 0.25),
                },
                Placement {
                    source: 0,
                    origin: Point::new(2.25, 1.0),
                },
                Placement {
                    source: 0,
                    origin: Point::new(4.75, 1.5),
                },
            ],
            positions: Vec::new(),
            paint: red,
            mark: mark(half),
        }),
    );
    check(&b);
}

#[test]
fn fixed_views_keep_their_size() {
    for (no_zoom, no_rotate) in [(true, false), (false, true), (true, true)] {
        let mut b = Build::new(6.0, 4.0);
        b.scale = 2.0;
        let red = b.rgb([1.0, 0.2, 0.0]);
        let group = b.transparency(PAGE, GroupAttributes::default(), mark(StateId(0)));
        b.scene.groups[group.index()].view = Some(pagura_interpret::ViewFit {
            anchor: Point::new(1.0, 1.0),
            no_zoom,
            no_rotate,
        });
        b.rect(group, [1.0, 1.0, 3.5, 2.75], red, mark(StateId(0)));
        check(&b);
    }
}
