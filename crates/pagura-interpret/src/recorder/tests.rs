use super::*;
use crate::device::{FontInfo, FontKey, Glyph, GlyphPaint, OutlineKey, TilingInfo};
use crate::model::{Color, Composite, MaskKind, Pen, TilingColors};
use crate::scene::Node;
use pagura_color::{ColorSpace, Components};
use pagura_core::{Position, Rect};
use pagura_document::Rotation;
use pagura_extract::TextRenderingMode;
use pagura_raster::{Adjustment, PathBuilder, Stroke, Transform};

fn page() -> PageInfo {
    PageInfo {
        object: pagura_core::ObjectId::new(3, 0),
        crop: Rect::from_corners(0.0, 0.0, 10.0, 10.0).unwrap(),
        rotation: Rotation::None,
        user_unit: 1.0,
        group: None,
        print_intent: false,
    }
}

fn square() -> Arc<Path> {
    let mut path = PathBuilder::new();
    path.rect(0.0, 0.0, 1.0, 1.0);
    Arc::new(path.finish())
}

fn paint(gray: f64) -> Arc<Paint> {
    paint_in(&Arc::new(ColorSpace::DeviceGray), gray)
}

fn paint_in(space: &Arc<ColorSpace>, gray: f64) -> Arc<Paint> {
    Arc::new(Paint::Solid(Color {
        space: Arc::clone(space),
        components: Components::from_slice(&[gray]),
    }))
}

fn state(composite: &Arc<Composite>) -> MarkState<'_> {
    MarkState {
        clip: None,
        composite,
        position: Position::Unknown,
        hidden: false,
    }
}

fn font() -> FontInfo {
    FontInfo {
        key: FontKey(1),
        object: None,
        base_name: None,
        vertical: false,
        kind: pagura_text::FontKind::Type1,
    }
}

fn glyph(outline: &Arc<Path>, x: u8) -> Glyph {
    Glyph {
        code: 1,
        outline_key: OutlineKey {
            font: FontKey(1),
            glyph: 1,
        },
        transform: Transform::new(10.0, 0.0, 0.0, 10.0, f64::from(x), 0.0),
        outline: Some(Arc::clone(outline)),
        text: None,
        position: Position::Buffer(usize::from(x)),
    }
}

fn run<'a>(
    font: &'a FontInfo,
    glyphs: &'a [Glyph],
    mode: TextRenderingMode,
    paint: &'a Arc<Paint>,
    pen: &'a Arc<Pen>,
    composite: &'a Arc<Composite>,
) -> GlyphRun<'a> {
    GlyphRun {
        font,
        glyphs,
        mode,
        paint: GlyphPaint {
            fill: mode.fills().then_some(paint),
            fill_state: mode.fills().then(|| state(composite)),
            stroke: mode.strokes().then_some((paint, pen)),
            stroke_state: mode.strokes().then(|| state(composite)),
        },
        hidden: false,
    }
}

fn pen() -> Arc<Pen> {
    Arc::new(Pen {
        stroke: Stroke::default(),
        transform: Transform::IDENTITY,
        adjustment: Adjustment::None,
    })
}

#[test]
fn consecutive_runs_of_one_paint_and_state_share_a_glyphs_node() {
    let mut recorder = SceneRecorder::new(Limits::default());
    recorder.begin_page(&page()).unwrap();
    let (outline, font, pen) = (square(), font(), pen());
    let space = Arc::new(ColorSpace::DeviceGray);
    let (black, gray) = (paint_in(&space, 0.0), paint_in(&space, 0.0));
    let composite = Arc::new(Composite::default());
    let half = Arc::new(Composite {
        alpha: 0.5,
        ..Composite::default()
    });
    let first = [glyph(&outline, 0), glyph(&outline, 2)];
    let second = [glyph(&outline, 4)];
    let fill = TextRenderingMode::Fill;
    recorder
        .glyphs(&run(&font, &first, fill, &black, &pen, &composite))
        .unwrap();
    // An equal paint built separately extends the node.
    recorder
        .glyphs(&run(&font, &second, fill, &gray, &pen, &composite))
        .unwrap();
    recorder
        .glyphs(&run(&font, &second, fill, &black, &pen, &half))
        .unwrap();
    recorder.end_page().unwrap();
    let scene = recorder.finish().unwrap();
    assert_eq!(scene.nodes.len(), 2);
    let Node::Glyphs(glyphs) = &scene.nodes[0] else {
        panic!("{:?}", scene.nodes[0])
    };
    assert_eq!(glyphs.sources.len(), 1);
    assert_eq!(glyphs.placements.len(), 3);
    assert_eq!(glyphs.position(2), Position::Buffer(4));
    assert_eq!(scene.marks(), 4);
}

#[test]
fn stroked_glyphs_become_strokes_and_filled_and_stroked_glyphs_groups() {
    let mut recorder = SceneRecorder::new(Limits::default());
    recorder.begin_page(&page()).unwrap();
    let (outline, font, pen) = (square(), font(), pen());
    let black = paint(0.0);
    let composite = Arc::new(Composite::default());
    let glyphs = [glyph(&outline, 0)];
    recorder
        .glyphs(&run(
            &font,
            &glyphs,
            TextRenderingMode::Stroke,
            &black,
            &pen,
            &composite,
        ))
        .unwrap();
    recorder
        .glyphs(&run(
            &font,
            &glyphs,
            TextRenderingMode::FillStroke,
            &black,
            &pen,
            &composite,
        ))
        .unwrap();
    recorder.end_page().unwrap();
    let scene = recorder.finish().unwrap();
    let kinds: Vec<_> = scene
        .nodes
        .iter()
        .map(|node| match node {
            Node::Fill(_) => "fill",
            Node::Stroke(_) => "stroke",
            Node::Group(_) => "group",
            _ => "other",
        })
        .collect();
    assert_eq!(kinds, ["stroke", "group", "fill", "stroke"]);
    let Node::Stroke(stroke) = &scene.nodes[0] else {
        panic!()
    };
    assert_eq!(
        stroke.path.segments().first(),
        Some(&pagura_raster::Segment::MoveTo(pagura_raster::Point::new(
            0.0, 0.0
        )))
    );
    assert_eq!(
        stroke.path.segments()[1],
        pagura_raster::Segment::LineTo(pagura_raster::Point::new(10.0, 0.0))
    );
    let Node::Group(group) = scene.nodes[1] else {
        panic!()
    };
    let group = scene.group(group).unwrap();
    assert!(group.attributes.knockout);
    assert_eq!(group.cause, GroupCause::FillStroke);
    assert!(group.cause.is_implicit());
}

#[test]
fn definitions_are_numbered_in_order_and_groups_balance() {
    let mut recorder = SceneRecorder::new(Limits::default());
    recorder.begin_page(&page()).unwrap();
    let tiling = TilingInfo {
        id: PatternId(1),
        bbox: Rect::from_corners(0.0, 0.0, 1.0, 1.0).unwrap(),
        step: [1.0, 1.0],
        matrix: Transform::IDENTITY,
        colors: TilingColors::Colored,
        object: None,
    };
    assert!(recorder.begin_tiling(&tiling).is_err());
    recorder
        .begin_tiling(&TilingInfo {
            id: PatternId(0),
            ..tiling
        })
        .unwrap();
    assert!(recorder.end_soft_mask().is_err());
    recorder.end_tiling().unwrap();
    recorder
        .begin_soft_mask(&SoftMaskInfo {
            id: MaskId(0),
            kind: MaskKind::Luminosity,
            group: GroupAttributes::default(),
            bbox: None,
            backdrop: None,
            transfer: None,
            position: Position::Unknown,
        })
        .unwrap();
    recorder.end_soft_mask().unwrap();
    recorder.end_page().unwrap();
    let scene = recorder.finish().unwrap();
    assert_eq!((scene.tilings.len(), scene.masks.len()), (1, 1));
    assert_eq!(scene.group(scene.root).unwrap().children, []);
    let unfinished = SceneRecorder::new(Limits::default());
    assert!(unfinished.finish().is_err());
}

#[test]
fn marks_are_charged_to_the_display_list_budget() {
    let mut recorder = SceneRecorder::new(Limits {
        display_commands: 1,
        ..Limits::default()
    });
    recorder.begin_page(&page()).unwrap();
    let (path, black) = (square(), paint(0.0));
    let composite = Arc::new(Composite::default());
    let mark = FillMark {
        path: &path,
        rule: FillRule::NonZero,
        paint: Some(&black),
        state: state(&composite),
    };
    recorder.fill(&mark).unwrap();
    let error = recorder.fill(&mark).unwrap_err();
    assert_eq!(error.kind, pagura_core::ErrorKind::Limit);
}
