//! Storage formats per buffer (ADR 0004 §5–§6), from whole content.

use crate::format::Format;
use crate::testing::{Build, check, mark, rect_path};
use pagura_color::Process;
use pagura_core::Limits;
use pagura_interpret::{
    BlendMode, GlyphSource, GlyphsNode, GroupAttributes, GroupId, MaskKind, Node, OutlineKey,
    Placement, StateId, Transfer,
};
use pagura_raster::{Point, Transform};
use std::sync::Arc;

const PAGE: GroupId = GroupId(0);

/// The buffers planned: the page, groups and soft masks.
fn planned(b: &Build) -> usize {
    b.prepare(&b.request(b.page(), 32), &Limits::default())
        .unwrap_or_else(|e| panic!("{e}"))
        .plan
        .groups
        .len()
}

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
fn knockout_groups_of_opaque_later_members_composite_as_source_over() {
    let mut b = Build::new(8.0, 6.0);
    let back = b.rgb([0.1, 0.5, 0.9]);
    let green = b.rgb([0.6, 0.73, 0.35]);
    let dark = b.rgb([0.45, 0.55, 0.24]);
    let half = b.state(|s| s.alpha = 0.5);
    b.rect(PAGE, [0.0, 0.0, 8.0, 6.0], back, mark(StateId(0)));
    // Fill and stroke of one path (11.7.4.4): a translucent fill under an
    // opaque stroke, non-isolated and isolated.
    for isolated in [false, true] {
        let attributes = GroupAttributes {
            isolated,
            knockout: true,
            space: None,
        };
        let both = b.transparency(PAGE, attributes, mark(StateId(0)));
        b.rect(both, [1.0, 1.0, 5.5, 4.5], green, mark(half));
        b.rect(both, [0.75, 0.75, 2.25, 5.0], dark, mark(StateId(0)));
    }
    let prepared = b
        .prepare(&b.request(b.page(), 32), &Limits::default())
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(prepared.plan.groups.len(), 1);
    check(&b);
    // A translucent member after the first knocks out.
    let knockout = GroupAttributes {
        isolated: false,
        knockout: true,
        space: None,
    };
    let group = b.transparency(PAGE, knockout, mark(StateId(0)));
    b.rect(group, [3.0, 2.0, 7.5, 5.5], dark, mark(StateId(0)));
    b.rect(group, [2.5, 1.5, 6.0, 4.0], green, mark(half));
    assert_eq!(formats(&b)[1..], [Format::F64]);
    check(&b);
    // So does a translucent first run of two glyphs.
    let mut b = Build::new(6.0, 4.0);
    let back = b.rgb([0.1, 0.5, 0.9]);
    let red = b.rgb([0.9, 0.2, 0.1]);
    let half = b.state(|s| s.alpha = 0.5);
    b.rect(PAGE, [0.0, 0.0, 6.0, 4.0], back, mark(StateId(0)));
    let text = b.transparency(PAGE, knockout, mark(StateId(0)));
    let source = GlyphSource {
        outline: rect_path([0.0, 0.0, 2.5, 2.5]),
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
            placements: (0..2_u32)
                .map(|i| Placement {
                    source: 0,
                    origin: Point::new(0.5 + 1.5 * f64::from(i), 0.5),
                })
                .collect(),
            positions: Vec::new(),
            paint: red,
            mark: mark(half),
        }),
    );
    assert_eq!(formats(&b)[1..], [Format::F64]);
    check(&b);
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

#[test]
fn groups_that_paint_nothing_leave_their_parents_narrow() {
    let knockout = GroupAttributes {
        isolated: false,
        knockout: true,
        space: None,
    };
    let invisible = |b: &mut Build, parent: GroupId| {
        let red = b.rgb([0.9, 0.2, 0.1]);
        let clear = b.state(|s| s.alpha = 0.0);
        let text = b.transparency(parent, knockout, mark(StateId(0)));
        b.rect(text, [1.0, 1.0, 4.0, 3.0], red, mark(clear));
        b.rect(text, [2.0, 0.0, 5.0, 2.0], red, mark(clear));
    };
    let mut b = Build::new(6.0, 4.0);
    let back = b.rgb([0.1, 0.5, 0.9]);
    b.rect(PAGE, [0.5, 0.5, 5.5, 3.5], back, mark(StateId(0)));
    // A knockout group of alpha-0 members changes only its parent's shape:
    // it is left out, and the page stays N16.
    invisible(&mut b, PAGE);
    assert_eq!(formats(&b), [Format::N16]);
    let prepared = b
        .prepare(&b.request(b.page(), 32), &Limits::default())
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(prepared.plan.groups.len(), 1);
    check(&b);
    // Under a knockout parent its shape knocks the parent's members out.
    let mut b = Build::new(6.0, 4.0);
    let back = b.rgb([0.1, 0.5, 0.9]);
    let half = b.state(|s| s.alpha = 0.5);
    let outer = b.transparency(PAGE, knockout, mark(StateId(0)));
    b.rect(outer, [0.5, 0.5, 5.5, 3.5], back, mark(half));
    invisible(&mut b, outer);
    let found = formats(&b);
    assert_eq!(found.len(), 3);
    assert!(found.iter().all(|&f| f == Format::F64));
    check(&b);
    // Alpha-0 marks still select transfer functions: with one on the page,
    // the group stays.
    let mut b = Build::new(6.0, 4.0);
    let back = b.rgb([0.1, 0.5, 0.9]);
    let identity = Arc::new(Transfer::All(Arc::new(
        pagura_color::Function::exponential(pagura_color::ExponentialFunction {
            domain: [0.0, 1.0],
            range: None,
            c0: vec![0.0],
            c1: vec![1.0],
            exponent: 1.0,
        })
        .unwrap_or_else(|e| panic!("{e}")),
    )));
    let transferred = b.state(|s| s.transfer = Some(identity));
    b.rect(PAGE, [0.0, 0.0, 6.0, 4.0], back, mark(transferred));
    invisible(&mut b, PAGE);
    let prepared = b
        .prepare(&b.request(b.page(), 32), &Limits::default())
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(prepared.plan.groups.len(), 2);
    check(&b);
}

#[test]
fn groups_of_one_store_fold_into_their_parents() {
    let isolated = GroupAttributes {
        isolated: true,
        ..GroupAttributes::default()
    };
    // A page of colour and a group of alpha 0.4 around one mark, whose
    // state `member` makes.
    let page = |attributes: GroupAttributes, member: &dyn Fn(&mut Build) -> StateId| {
        let mut b = Build::new(8.0, 6.0);
        let back = b.rgb([0.9, 0.8, 0.3]);
        let ink = b.rgb([0.2, 0.4, 0.7]);
        b.rect(PAGE, [0.0, 0.0, 8.0, 6.0], back, mark(StateId(0)));
        let alpha = b.state(|s| s.alpha = 0.4);
        let group = b.transparency(PAGE, attributes, mark(alpha));
        let state = member(&mut b);
        b.rect(group, [0.5, 1.25, 6.75, 4.5], ink, mark(state));
        (b, group)
    };
    let opaque = |_: &mut Build| StateId(0);
    // Its member composites at 0.4 times its own opacity, with no buffer.
    for attributes in [GroupAttributes::default(), isolated] {
        let (b, _) = page(attributes, &opaque);
        assert_eq!(planned(&b), 1);
        assert_eq!(formats(&b), [Format::N16]);
        check(&b);
    }
    // So does a translucent member under its own soft mask.
    let masked = |b: &mut Build| {
        let ink = b.rgb([0.8, 0.1, 0.1]);
        let (mask, group) = b.mask(MaskKind::Luminosity, GroupAttributes::default(), None);
        b.rect(group, [1.0, 1.0, 7.0, 5.0], ink, mark(StateId(0)));
        b.state(|s| {
            s.alpha = 0.7;
            s.soft_mask = Some(mask);
        })
    };
    let (b, _) = page(GroupAttributes::default(), &masked);
    assert_eq!(planned(&b), 2);
    check(&b);
    // A second member, and a member of another blend mode, keep the
    // group's buffer.
    let (mut b, group) = page(GroupAttributes::default(), &opaque);
    let ink = b.rgb([0.1, 0.6, 0.2]);
    b.rect(group, [3.0, 0.25, 7.75, 3.0], ink, mark(StateId(0)));
    assert_eq!(planned(&b), 2);
    check(&b);
    let multiply = |b: &mut Build| b.state(|s| s.blend = BlendMode::Multiply);
    let (b, _) = page(GroupAttributes::default(), &multiply);
    assert_eq!(planned(&b), 2);
    check(&b);
    // A group under a soft mask of its own folds too, its member taking
    // the mask, unless the member has one.
    let (mut b, group) = page(GroupAttributes::default(), &opaque);
    let ink = b.rgb([0.8, 0.1, 0.1]);
    let (mask, mask_group) = b.mask(MaskKind::Alpha, GroupAttributes::default(), None);
    b.rect(mask_group, [1.0, 1.0, 7.0, 5.0], ink, mark(StateId(0)));
    let masked_group = b.state(|s| {
        s.alpha = 0.4;
        s.soft_mask = Some(mask);
    });
    b.scene.groups[group.index()].mark.state = masked_group;
    assert_eq!(planned(&b), 2);
    check(&b);
    let (mut b, group) = page(GroupAttributes::default(), &masked);
    let ink = b.rgb([0.8, 0.1, 0.1]);
    let (mask, mask_group) = b.mask(MaskKind::Alpha, GroupAttributes::default(), None);
    b.rect(mask_group, [0.5, 2.0, 6.0, 5.5], ink, mark(StateId(0)));
    let masked_group = b.state(|s| {
        s.alpha = 0.4;
        s.soft_mask = Some(mask);
    });
    b.scene.groups[group.index()].mark.state = masked_group;
    assert_eq!(planned(&b), 4);
    check(&b);
    // Under a knockout parent the group stays one element of its own.
    let mut b = Build::new(8.0, 6.0);
    let ink = b.rgb([0.2, 0.4, 0.7]);
    let knockout = GroupAttributes {
        knockout: true,
        ..GroupAttributes::default()
    };
    let knockout = b.transparency(PAGE, knockout, mark(StateId(0)));
    let half = b.state(|s| s.alpha = 0.5);
    b.rect(knockout, [0.0, 0.0, 5.0, 4.0], ink, mark(half));
    let alpha = b.state(|s| s.alpha = 0.4);
    let group = b.transparency(knockout, GroupAttributes::default(), mark(alpha));
    b.rect(group, [2.0, 1.0, 7.0, 5.0], ink, mark(StateId(0)));
    assert_eq!(planned(&b), 3);
    check(&b);
}

#[test]
fn groups_of_one_group_fold_and_alpha_as_shape_where_no_knockout_reads_it() {
    let isolated = GroupAttributes {
        isolated: true,
        ..GroupAttributes::default()
    };
    // A group of one group folds into it: a translucent group around a
    // soft-masked group with alpha as shape around one mark folds both;
    // with two marks the inner group keeps its buffer, composited at 0.3
    // times its alpha through its mask.
    let nested = |second: bool| {
        let mut b = Build::new(8.0, 6.0);
        let back = b.rgb([0.9, 0.8, 0.3]);
        let ink = b.rgb([0.2, 0.4, 0.7]);
        let red = b.rgb([0.8, 0.1, 0.1]);
        b.rect(PAGE, [0.0, 0.0, 8.0, 6.0], back, mark(StateId(0)));
        let (mask, mask_group) = b.mask(MaskKind::Luminosity, GroupAttributes::default(), None);
        b.rect(mask_group, [1.0, 1.0, 7.0, 5.0], red, mark(StateId(0)));
        let alpha = b.state(|s| s.alpha = 0.3);
        let outer = b.transparency(PAGE, isolated, mark(alpha));
        let masked = b.state(|s| {
            s.alpha_is_shape = true;
            s.soft_mask = Some(mask);
        });
        let inner = b.transparency(outer, GroupAttributes::default(), mark(masked));
        let shaped = b.state(|s| s.alpha_is_shape = true);
        b.rect(inner, [0.5, 1.25, 6.75, 4.5], ink, mark(shaped));
        if second {
            b.rect(inner, [3.0, 0.25, 7.75, 3.0], red, mark(StateId(0)));
        }
        b
    };
    let b = nested(false);
    assert_eq!(planned(&b), 2);
    check(&b);
    let b = nested(true);
    assert_eq!(planned(&b), 3);
    check(&b);
    // Alpha as shape folds only where no knockout above reads shape.
    let ancestor = |state: &dyn Fn(&mut Build) -> StateId| {
        let mut b = Build::new(8.0, 6.0);
        let ink = b.rgb([0.2, 0.4, 0.7]);
        let knockout = GroupAttributes {
            knockout: true,
            ..GroupAttributes::default()
        };
        let knockout = b.transparency(PAGE, knockout, mark(StateId(0)));
        let half = b.state(|s| s.alpha = 0.5);
        b.rect(knockout, [0.0, 0.0, 5.0, 4.0], ink, mark(half));
        let parent = b.transparency(knockout, isolated, mark(StateId(0)));
        let state = state(&mut b);
        let group = b.transparency(parent, GroupAttributes::default(), mark(state));
        b.rect(group, [2.0, 1.0, 7.0, 5.0], ink, mark(StateId(0)));
        b
    };
    let b = ancestor(&|b: &mut Build| b.state(|s| s.alpha = 0.4));
    assert_eq!(planned(&b), 3);
    check(&b);
    let b = ancestor(&|b: &mut Build| {
        b.state(|s| {
            s.alpha = 0.4;
            s.alpha_is_shape = true;
        })
    });
    assert_eq!(planned(&b), 4);
    check(&b);
}
