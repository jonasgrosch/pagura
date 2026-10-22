//! Storage formats per buffer (ADR 0004 §5–§6), from whole content.

use crate::format::Format;
use crate::testing::{Build, check, mark, rect_path};
use pagura_color::Process;
use pagura_core::Limits;
use pagura_interpret::{
    BlendMode, GlyphSource, GlyphsNode, GroupAttributes, GroupId, MaskKind, Node, OutlineKey,
    Placement, StateId,
};
use pagura_raster::{Point, Transform};

const PAGE: GroupId = GroupId(0);

/// The formats of the page and every buffer that has items, in plan order.
fn formats(b: &Build) -> Vec<Format> {
    let request = b.request(b.page(), 32);
    let prepared = b
        .prepare(&request, &Limits::default())
        .unwrap_or_else(|e| panic!("{e}"));
    prepared
        .plan
        .groups
        .iter()
        .enumerate()
        .filter(|(i, g)| *i == 0 || !g.items.is_empty())
        .map(|(_, g)| g.format)
        .collect()
}

#[test]
fn opaque_pages_store_n16_and_translucent_ones_f32() {
    let mut b = Build::new(6.0, 4.0);
    let red = b.rgb([1.0, 0.0, 0.0]);
    b.rect(PAGE, [0.5, 0.5, 4.25, 3.5], red, mark(StateId(0)));
    b.rect(PAGE, [1.5, 0.25, 6.0, 2.75], red, mark(StateId(0)));
    assert_eq!(formats(&b), [Format::N16]);
    check(&b);
    // One translucent store fits N16's room; two do not.
    let half = b.state(|s| s.alpha = 0.5);
    b.rect(PAGE, [0.0, 0.0, 3.0, 3.0], red, mark(half));
    assert_eq!(formats(&b), [Format::N16]);
    check(&b);
    b.rect(PAGE, [2.0, 1.0, 5.0, 4.0], red, mark(half));
    assert_eq!(formats(&b), [Format::F32]);
    check(&b);
    // Dynamic stores that share no pixel: N16 admits one at each pixel.
    let mut b = Build::new(8.0, 4.0);
    let red = b.rgb([1.0, 0.0, 0.0]);
    let half = b.state(|s| s.alpha = 0.5);
    b.rect(PAGE, [0.5, 0.5, 7.5, 3.5], red, mark(StateId(0)));
    b.rect(PAGE, [0.0, 0.0, 2.5, 4.0], red, mark(half));
    b.rect(PAGE, [5.0, 0.0, 8.0, 4.0], red, mark(half));
    assert_eq!(formats(&b), [Format::N16]);
    check(&b);
    // Bounds within a pixel of each other may share it.
    b.rect(PAGE, [3.25, 1.0, 4.5, 2.0], red, mark(half));
    assert_eq!(formats(&b), [Format::F32]);
    // A translucent group's result is one dynamic store over its content.
    let mut b = Build::new(8.0, 4.0);
    let red = b.rgb([1.0, 0.0, 0.0]);
    let half = b.state(|s| s.alpha = 0.5);
    b.rect(PAGE, [0.5, 0.5, 7.5, 3.5], red, mark(StateId(0)));
    let group = b.transparency(PAGE, GroupAttributes::default(), mark(half));
    b.rect(group, [0.0, 0.0, 2.5, 4.0], red, mark(StateId(0)));
    b.rect(PAGE, [5.0, 0.0, 8.0, 4.0], red, mark(half));
    assert_eq!(formats(&b)[0], Format::N16);
    check(&b);
    b.rect(group, [5.5, 1.0, 6.5, 2.0], red, mark(StateId(0)));
    assert_eq!(formats(&b)[0], Format::F32);
}

#[test]
fn other_blend_modes_and_spaces_stay_f64() {
    let mut b = Build::new(4.0, 4.0);
    let red = b.rgb([1.0, 0.0, 0.0]);
    let multiply = b.state(|s| s.blend = BlendMode::Multiply);
    b.rect(PAGE, [0.5, 0.5, 3.0, 3.0], red, mark(multiply));
    assert_eq!(formats(&b), [Format::F64]);
    let mut b = Build::new(4.0, 4.0);
    b.scene.groups[0].attributes.space = Some(Process::Gray);
    assert_eq!(formats(&b), [Format::F64]);
}

#[test]
fn translucent_groups_and_masks_store_f32_in_narrow_parents() {
    let mut b = Build::new(8.0, 6.0);
    let red = b.rgb([1.0, 0.0, 0.0]);
    let half = b.state(|s| s.alpha = 0.5);
    b.rect(PAGE, [0.0, 0.0, 8.0, 6.0], red, mark(StateId(0)));
    let group = b.transparency(
        PAGE,
        GroupAttributes {
            isolated: true,
            ..GroupAttributes::default()
        },
        mark(half),
    );
    b.rect(group, [0.5, 0.5, 6.0, 4.0], red, mark(half));
    b.rect(group, [2.5, 1.5, 7.0, 5.0], red, mark(half));
    // The group reaches the page as its one dynamic store.
    assert_eq!(formats(&b), [Format::N16, Format::F32]);
    check(&b);
    let (mask, mask_group) = b.mask(MaskKind::Luminosity, GroupAttributes::default(), None);
    b.rect(mask_group, [1.0, 1.0, 7.0, 5.0], red, mark(half));
    let masked = b.state(|s| s.soft_mask = Some(mask));
    b.rect(PAGE, [0.25, 0.25, 7.75, 5.75], red, mark(masked));
    assert_eq!(formats(&b), [Format::F32, Format::F32, Format::F32]);
    check(&b);
}

#[test]
fn knockout_and_backdrop_reading_groups_stay_f64() {
    let mut b = Build::new(6.0, 4.0);
    let red = b.rgb([1.0, 0.0, 0.0]);
    let half = b.state(|s| s.alpha = 0.5);
    let knockout = b.transparency(
        PAGE,
        GroupAttributes {
            isolated: true,
            knockout: true,
            space: None,
        },
        mark(StateId(0)),
    );
    b.rect(knockout, [0.5, 0.5, 4.0, 3.0], red, mark(half));
    b.rect(knockout, [1.5, 1.0, 5.5, 3.5], red, mark(half));
    // An F64 child injects little: the page stays N16.
    assert_eq!(formats(&b), [Format::N16, Format::F64]);
    check(&b);
    // A non-isolated group of other blend modes reads its parent.
    let multiply = b.state(|s| s.blend = BlendMode::Multiply);
    let reader = b.transparency(PAGE, GroupAttributes::default(), mark(half));
    b.rect(reader, [0.0, 0.0, 3.0, 4.0], red, mark(multiply));
    assert_eq!(formats(&b), [Format::F64, Format::F64, Format::F64]);
    check(&b);
}

#[test]
fn plain_groups_composite_as_their_members() {
    let mut b = Build::new(6.0, 4.0);
    let red = b.rgb([1.0, 0.0, 0.0]);
    let multiply = b.state(|s| s.blend = BlendMode::Multiply);
    let group = b.transparency(PAGE, GroupAttributes::default(), mark(StateId(0)));
    b.rect(group, [0.5, 0.5, 4.0, 3.0], red, mark(StateId(0)));
    b.rect(group, [1.5, 1.0, 5.5, 3.5], red, mark(multiply));
    // The non-isolated plain group is flattened into the page.
    assert_eq!(formats(&b), [Format::F64]);
    let prepared = b
        .prepare(&b.request(b.page(), 32), &Limits::default())
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(prepared.plan.groups[0].items.len(), 2);
    check(&b);
}

#[test]
fn the_f32_count_bound_sends_long_faint_stacks_to_f64() {
    let mut b = Build::new(4.0, 4.0);
    let red = b.rgb([1.0, 0.0, 0.0]);
    let faint = b.state(|s| s.alpha = 3.0 / 134_217_728.0);
    let source = GlyphSource {
        outline: rect_path([0.0, 0.0, 1.0, 1.0]),
        key: OutlineKey {
            font: pagura_interpret::FontKey(1),
            glyph: 1,
        },
        linear: Transform::IDENTITY,
    };
    let placements = |count: u32| {
        (0..count)
            .map(|i| Placement {
                source: 0,
                origin: Point::new(f64::from(i % 3), f64::from(i % 2)),
            })
            .collect::<Vec<_>>()
    };
    let glyphs = |count: u32| {
        Node::Glyphs(GlyphsNode {
            sources: vec![source.clone()],
            placements: placements(count),
            positions: Vec::new(),
            paint: red,
            mark: mark(faint),
        })
    };
    b.node(PAGE, glyphs(1_000));
    assert_eq!(formats(&b), [Format::F32]);
    b.scene.nodes.clear();
    b.scene.groups[0].children.clear();
    b.node(PAGE, glyphs(70_000));
    assert_eq!(formats(&b), [Format::F64]);
}

#[test]
fn knockout_groups_of_opaque_members_fold_with_their_plain_groups() {
    let knockout = GroupAttributes {
        isolated: false,
        knockout: true,
        space: None,
    };
    let mut b = Build::new(12.0, 6.0);
    let back = b.rgb([0.1, 0.5, 0.9]);
    let red = b.rgb([0.9, 0.2, 0.1]);
    let black = b.rgb([0.0, 0.0, 0.0]);
    b.rect(PAGE, [0.0, 0.0, 12.0, 6.0], back, mark(StateId(0)));
    // A text object under text knockout: its glyphs, a Type 3 glyph (a plain
    // group) and fill-and-stroke text (a knockout group), all opaque.
    let text = b.transparency(PAGE, knockout, mark(StateId(0)));
    let source = GlyphSource {
        outline: rect_path([0.0, 0.0, 1.5, 2.5]),
        key: OutlineKey {
            font: pagura_interpret::FontKey(1),
            glyph: 2,
        },
        linear: Transform::IDENTITY,
    };
    b.node(
        text,
        Node::Glyphs(GlyphsNode {
            sources: vec![source],
            placements: (0..4_u32)
                .map(|i| Placement {
                    source: 0,
                    origin: Point::new(0.5 + 1.25 * f64::from(i), 0.5),
                })
                .collect(),
            positions: Vec::new(),
            paint: red,
            mark: mark(StateId(0)),
        }),
    );
    let type3 = b.transparency(text, GroupAttributes::default(), mark(StateId(0)));
    b.rect(type3, [6.25, 0.5, 8.5, 3.0], black, mark(StateId(0)));
    b.rect(type3, [7.0, 1.25, 9.75, 4.5], red, mark(StateId(0)));
    let both = b.transparency(text, knockout, mark(StateId(0)));
    b.rect(both, [2.5, 3.0, 6.0, 5.5], red, mark(StateId(0)));
    b.rect(both, [2.25, 2.75, 6.25, 5.75], black, mark(StateId(0)));
    assert_eq!(formats(&b), [Format::N16]);
    let prepared = b
        .prepare(&b.request(b.page(), 32), &Limits::default())
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(prepared.plan.groups.len(), 1);
    check(&b);
    // A translucent member makes the text object knock out: its groups are
    // members of their own.
    let half = b.state(|s| s.alpha = 0.5);
    b.rect(text, [5.0, 2.0, 11.5, 3.5], black, mark(half));
    let found = formats(&b);
    assert_eq!(found.len(), 4);
    assert!(found.iter().all(|&f| f == Format::F64));
    check(&b);
    // Opaque members that blend otherwise than Normal knock out too.
    let mut b = Build::new(6.0, 4.0);
    let back = b.rgb([0.1, 0.5, 0.9]);
    let a = b.rgb([0.9, 0.2, 0.1]);
    let c = b.rgb([0.3, 0.9, 0.2]);
    let multiply = b.state(|s| s.blend = BlendMode::Multiply);
    b.rect(PAGE, [0.0, 0.0, 6.0, 4.0], back, mark(StateId(0)));
    let group = b.transparency(PAGE, knockout, mark(StateId(0)));
    b.rect(group, [0.5, 0.5, 4.25, 3.25], a, mark(multiply));
    b.rect(group, [2.75, 1.5, 6.0, 4.0], c, mark(multiply));
    check(&b);
}
