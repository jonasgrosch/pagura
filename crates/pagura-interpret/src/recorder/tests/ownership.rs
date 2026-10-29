use super::*;
use crate::scene::{GlyphSource, Placement};
use pagura_core::ErrorKind;

fn allowance(recorder: &mut SceneRecorder, remaining: usize) {
    let used = recorder.bytes.used();
    let mut bytes = Meter::new(used + remaining, "scene byte budget exceeded");
    bytes.charge(used).unwrap();
    recorder.bytes = bytes;
}

fn filled(
    recorder: &mut SceneRecorder,
    path: &Arc<Path>,
    black: &Arc<Paint>,
    composite: &Arc<Composite>,
) {
    recorder
        .fill(&FillMark {
            path,
            rule: FillRule::NonZero,
            paint: Some(black),
            state: state(composite),
        })
        .unwrap();
}

#[test]
fn node_growth_reserves_the_old_and_replacement_buffers_before_mutation() {
    let mut recorder = SceneRecorder::new(Limits::default());
    recorder.begin_page(&page()).unwrap();
    let (path, black, composite) = (square(), paint(0.0), Arc::new(Composite::default()));
    // Fill the node buffer, so that the next push must grow it.
    filled(&mut recorder, &path, &black, &composite);
    while recorder.nodes.len() < recorder.nodes.capacity() {
        filled(&mut recorder, &path, &black, &composite);
    }
    let node = recorder.nodes[0].clone();
    let full = recorder.nodes.len();
    let old = (
        recorder.nodes.capacity(),
        recorder.groups[0].children.capacity(),
    );
    // Isolate inline growth; the copied node's shared payload is charged first.
    if let Node::Fill(fill) = &node {
        recorder.charge_path(&fill.path).unwrap();
    }
    allowance(&mut recorder, (full + 1) * size_of::<Node>() - 1);
    let used = recorder.bytes.used();
    let error = recorder.push(node, 0).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
    assert_eq!(
        (recorder.nodes.len(), recorder.groups[0].children.len()),
        (full, full)
    );
    assert_eq!(
        (
            recorder.nodes.capacity(),
            recorder.groups[0].children.capacity()
        ),
        old
    );
    assert_eq!(recorder.bytes.used(), used);
}

#[test]
fn retained_scene_bytes_exclude_recording_indexes_and_stack() {
    let mut recorder = SceneRecorder::new(Limits::default());
    recorder.begin_page(&page()).unwrap();
    let path = square();
    let black = paint(0.0);
    let composite = Arc::new(Composite::default());
    for _ in 0..3 {
        filled(&mut recorder, &path, &black, &composite);
    }
    recorder.end_page().unwrap();
    let peak = recorder.bytes.used();
    let scene = recorder.finish().unwrap();
    let arc = 2 * size_of::<usize>();
    let payload = arc + size_of::<Path>() + path.retained_bytes();
    let expected = size_of::<Scene>()
        + scene.nodes.capacity() * size_of::<Node>()
        + scene.groups.capacity() * size_of::<Group>()
        + scene.groups[0].children.capacity() * size_of::<NodeId>()
        + scene.states.capacity() * size_of::<Arc<Composite>>()
        + arc
        + size_of::<Composite>()
        + scene.paints.capacity() * size_of::<Arc<Paint>>()
        + arc
        + size_of::<Paint>()
        + arc
        + size_of::<ColorSpace>()
        + 3 * payload;
    assert_eq!(scene.bytes, expected);
    assert!(peak > scene.bytes, "recording indexes were temporary");
    assert_eq!(scene.states.len(), 1, "equal state owners are interned");
    assert_eq!(scene.paints.len(), 1);
}

#[test]
fn glyph_positions_reserve_before_growth() {
    let mut recorder = SceneRecorder::new(Limits::default());
    recorder.begin_page(&page()).unwrap();
    let (outline, font, pen) = (square(), font(), pen());
    let black = paint(0.0);
    let composite = Arc::new(Composite::default());
    let positions = |recorder: &SceneRecorder| match &recorder.nodes[..] {
        [Node::Glyphs(node)] => (node.positions.len(), node.positions.capacity()),
        _ => (0, 1),
    };
    // Runs of one paint and state join a node until its positions are full.
    let mut x = 0;
    while positions(&recorder).0 < positions(&recorder).1 {
        recorder
            .glyphs(&run(
                &font,
                &[glyph(&outline, x)],
                TextRenderingMode::Fill,
                &black,
                &pen,
                &composite,
            ))
            .unwrap();
        x += 2;
    }
    let (full, _) = positions(&recorder);
    allowance(&mut recorder, (full + 1) * size_of::<(u32, Position)>() - 1);
    let second = [glyph(&outline, x)];
    let error = recorder
        .glyphs(&run(
            &font,
            &second,
            TextRenderingMode::Fill,
            &black,
            &pen,
            &composite,
        ))
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
    let Node::Glyphs(node) = &recorder.nodes[0] else {
        panic!("glyph node")
    };
    assert_eq!(
        (
            node.positions.len(),
            node.positions.capacity(),
            node.placements.len()
        ),
        (full, full, full)
    );
}

#[test]
fn glyph_source_and_position_capacity_are_retained_but_hull_indexes_are_not() {
    let mut recorder = SceneRecorder::new(Limits::default());
    recorder.begin_page(&page()).unwrap();
    let (outline, font, pen) = (square(), font(), pen());
    let black = paint(0.0);
    let composite = Arc::new(Composite::default());
    let glyphs = [glyph(&outline, 0), glyph(&outline, 2), glyph(&outline, 4)];
    recorder
        .glyphs(&run(
            &font,
            &glyphs,
            TextRenderingMode::Fill,
            &black,
            &pen,
            &composite,
        ))
        .unwrap();
    let temporary = recorder.glyphs.retained_bytes(&recorder.bytes).unwrap();
    assert!(temporary > 0);
    recorder.end_page().unwrap();
    let peak = recorder.bytes.used();
    let scene = recorder.finish().unwrap();
    let Node::Glyphs(node) = &scene.nodes[0] else {
        panic!("glyph node")
    };
    assert_eq!(node.sources.len(), 1);
    assert_eq!(node.position(2), Position::Buffer(4));
    let arc = 2 * size_of::<usize>();
    let expected = size_of::<Scene>()
        + scene.nodes.capacity() * size_of::<Node>()
        + scene.groups.capacity() * size_of::<Group>()
        + scene.groups[0].children.capacity() * size_of::<NodeId>()
        + scene.states.capacity() * size_of::<Arc<Composite>>()
        + arc
        + size_of::<Composite>()
        + scene.paints.capacity() * size_of::<Arc<Paint>>()
        + arc
        + size_of::<Paint>()
        + arc
        + size_of::<ColorSpace>()
        + node.sources.capacity() * size_of::<GlyphSource>()
        + node.placements.capacity() * size_of::<Placement>()
        + node.positions.capacity() * size_of::<(u32, Position)>()
        + arc
        + size_of::<Path>()
        + outline.retained_bytes();
    assert_eq!(scene.bytes, expected);
    assert!(peak >= scene.bytes + temporary);
}

#[test]
fn transformed_stroke_paths_refuse_before_copying_the_geometry() {
    let mut recorder = SceneRecorder::new(Limits::default());
    recorder.begin_page(&page()).unwrap();
    let (outline, font, pen) = (square(), font(), pen());
    let black = paint(0.0);
    let composite = Arc::new(Composite::default());
    let glyphs = [glyph(&outline, 0)];
    recorder.paint(Some(&black)).unwrap();
    recorder.mark(&state(&composite)).unwrap();
    allowance(
        &mut recorder,
        owned::ARC + size_of::<Path>() + size_of_val(outline.segments()) - 1,
    );
    assert_eq!(
        recorder
            .glyphs(&run(
                &font,
                &glyphs,
                TextRenderingMode::Stroke,
                &black,
                &pen,
                &composite
            ))
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
    assert!(recorder.nodes.is_empty());
}

#[test]
fn tiling_rectangles_are_reserved_before_path_construction() {
    let mut recorder = SceneRecorder::new(Limits::default());
    recorder.begin_page(&page()).unwrap();
    allowance(
        &mut recorder,
        owned::ARC + size_of::<Path>() + 5 * size_of::<pagura_raster::Segment>() - 1,
    );
    let error = recorder
        .begin_tiling(&TilingInfo {
            id: PatternId(0),
            bbox: page().crop,
            step: [1.0, 1.0],
            matrix: Transform::IDENTITY,
            colors: TilingColors::Colored,
            spacing: crate::model::TilingSpacing::Constant,
            object: None,
        })
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
    assert_eq!(
        (
            recorder.groups.len(),
            recorder.tilings.len(),
            recorder.stack.len()
        ),
        (1, 0, 1)
    );
}

#[test]
fn clip_identity_roots_are_temporary_but_retained_paths_stay_charged() {
    let mut recorder = SceneRecorder::new(Limits::default());
    recorder.begin_page(&page()).unwrap();
    let path = square();
    let black = paint(0.0);
    let composite = Arc::new(Composite::default());
    let clip = Arc::new(crate::Clip::new(None, Arc::clone(&path), FillRule::NonZero));
    recorder
        .fill(&FillMark {
            path: &path,
            rule: FillRule::NonZero,
            paint: Some(&black),
            state: MarkState {
                clip: Some(&clip),
                ..state(&composite)
            },
        })
        .unwrap();
    recorder.end_page().unwrap();
    let before = recorder.bytes.used();
    let scene = recorder.finish().unwrap();
    let without_clip = {
        let mut plain = SceneRecorder::new(Limits::default());
        plain.begin_page(&page()).unwrap();
        filled(&mut plain, &path, &black, &composite);
        plain.end_page().unwrap();
        plain.finish().unwrap().bytes
    };
    assert_eq!(scene.clips.len(), 1);
    assert_eq!(
        scene.bytes - without_clip,
        scene.clips.capacity() * size_of::<crate::ClipNode>()
            + owned::ARC
            + size_of::<Path>()
            + path.retained_bytes()
    );
    assert!(before > scene.bytes + owned::ARC + size_of::<crate::Clip>());
}

#[test]
fn a_profile_shared_by_paints_is_charged_once() {
    let fixture = pagura_testkit::fixture("color", "lut16-cmyk-xyz.icc");
    let parse = || {
        Arc::new(
            pagura_color::Profile::parse(&fixture, 4, &Limits::default())
                .unwrap()
                .unwrap(),
        )
    };
    let icc = |profile: &Arc<pagura_color::Profile>| {
        Arc::new(ColorSpace::Icc(Arc::new(pagura_color::IccSpace {
            profile: Some(Arc::clone(profile)),
            alternate: Arc::new(ColorSpace::DeviceCmyk),
            components: 4,
            range: [[0.0, 1.0]; 4],
            object: None,
        })))
    };
    let cyan = |space: &Arc<ColorSpace>, c: f64| {
        Arc::new(Paint::Solid(Color {
            space: Arc::clone(space),
            components: Components::from_slice(&[c, 0.0, 0.0, 0.0]),
        }))
    };
    let (shared, other) = (parse(), parse());
    let charge = owned::ARC + shared.retained_bytes();
    let (path, composite) = (square(), Arc::new(Composite::default()));
    let space = icc(&shared);
    // A hundred paints, in one space and in others over the same profile.
    let record = |space_of: &dyn Fn(u8) -> Arc<ColorSpace>| {
        let mut recorder = SceneRecorder::new(Limits::default());
        recorder.begin_page(&page()).unwrap();
        for i in 0..100_u8 {
            let paint = cyan(&space_of(i), f64::from(i) / 128.0);
            filled(&mut recorder, &path, &paint, &composite);
        }
        recorder
    };
    let device = record(&|_| Arc::new(ColorSpace::DeviceCmyk));
    let mut profiled = record(&|i| {
        if i % 2 == 0 {
            Arc::clone(&space)
        } else {
            icc(&shared)
        }
    });
    // They differ by their ICCBased records, the profile once and its index.
    let record = owned::paint(&cyan(&space, 0.0))
        - owned::paint(&cyan(&Arc::new(ColorSpace::DeviceCmyk), 0.0));
    let index = profiled
        .interner
        .profiles
        .index_bytes(&profiled.bytes)
        .unwrap();
    assert_eq!(
        profiled.bytes.used() - device.bytes.used(),
        100 * record + charge + index
    );
    // Another profile is charged in full.
    filled(&mut profiled, &path, &cyan(&icc(&other), 0.5), &composite);
    assert_eq!(profiled.interner.profiles.len(), 2);
}
