use super::*;
use crate::device::{FontKey, Glyph, GlyphPaint, MarkedText, OutlineKey};
use pagura_core::{Name, Position, Rect};
use pagura_document::Rotation;
use pagura_extract::{Quad, TextRenderingMode};
use pagura_raster::{Point, Transform};

fn font(key: u64) -> FontInfo {
    FontInfo {
        key: FontKey(key),
        object: Some(ObjectId::new(u32::try_from(key).unwrap(), 0)),
        base_name: Some(Name::new(b"Font")),
        vertical: false,
        kind: pagura_text::FontKind::Type1,
    }
}

fn glyph(text: &str) -> Glyph {
    Glyph {
        code: 65,
        outline_key: OutlineKey {
            font: FontKey(1),
            glyph: 65,
        },
        transform: Transform::IDENTITY,
        outline: None,
        text: Some(Box::new(PositionedGlyph {
            unicode: Some(text.into()),
            font: 0,
            size: 12.0,
            writing_mode: pagura_text::WritingMode::Horizontal,
            quad: Quad([Point::default(); 4]),
            origin: Point::default(),
            axis: Point::new(1.0, 0.0),
            em: 1.0,
            advance_end: Point::new(1.0, 0.0),
            render_mode: TextRenderingMode::Fill,
            hidden: false,
            marked_content: Arc::from([]),
            show: 0,
        })),
        position: Position::Unknown,
    }
}

fn show(device: &mut TextDevice, font: &FontInfo, glyphs: &[Glyph]) {
    device
        .glyphs(&GlyphRun {
            font,
            glyphs,
            mode: TextRenderingMode::Fill,
            paint: GlyphPaint {
                fill: None,
                fill_state: None,
                stroke: None,
                stroke_state: None,
            },
            hidden: false,
        })
        .unwrap();
}

#[test]
fn glyphs_record_fonts_strings_marked_content_and_objects() {
    let mut device = TextDevice::new(Limits::default());
    let page = ObjectId::new(3, 0);
    device
        .begin_page(&PageInfo {
            object: page,
            crop: Rect::from_corners(0.0, 0.0, 10.0, 10.0).unwrap(),
            rotation: Rotation::None,
            user_unit: 1.0,
            group: None,
            print_intent: false,
        })
        .unwrap();
    let (first, second) = (font(1), font(2));
    device.select_font(&first).unwrap();
    device.select_font(&second).unwrap();
    device.select_font(&first).unwrap();
    show(&mut device, &first, &[glyph("a")]);
    let text = MarkedText {
        mcid: Some(4),
        actual_text: Some("x".into()),
        alt: None,
    };
    device
        .marked(&Marked::Begin {
            tag: &Name::new(b"Span"),
            properties: None,
            hidden: false,
            text: Some(&text),
        })
        .unwrap();
    let form = ObjectId::new(9, 0);
    device
        .marked(&Marked::ObjectBegin {
            object: form,
            hidden: true,
        })
        .unwrap();
    show(&mut device, &second, &[glyph("b"), glyph("c")]);
    device.marked(&Marked::ObjectEnd).unwrap();
    device.marked(&Marked::End).unwrap();
    show(&mut device, &second, &[]);
    let text = device.finish().unwrap();
    assert_eq!(text.fonts.len(), 2);
    let fonts: Vec<_> = text.glyphs.iter().map(|g| g.font).collect();
    assert_eq!(fonts, [0, 1, 1]);
    let shows: Vec<_> = text.glyphs.iter().map(|g| g.show).collect();
    assert_eq!(shows, [1, 2, 2]);
    assert_eq!(text.marked.len(), 1);
    assert_eq!(text.marked[0].glyphs, 1..3);
    assert_eq!(text.marked[0].content.container, Some(page));
    assert_eq!(text.marked[0].content.mcid, Some(4));
    assert_eq!(text.glyphs[1].marked_content.len(), 1);
    assert!(text.glyphs[0].marked_content.is_empty());
    assert_eq!(text.objects.len(), 1);
    assert_eq!(
        (text.objects[0].object, text.objects[0].glyphs.clone(), text.objects[0].hidden),
        (form, 1..3, true)
    );
}

#[test]
fn unbalanced_structure_and_unknown_fonts_are_errors() {
    let mut device = TextDevice::new(Limits::default());
    show_error(&mut device);
    let mut device = TextDevice::new(Limits::default());
    device
        .marked(&Marked::ObjectBegin {
            object: ObjectId::new(1, 0),
            hidden: false,
        })
        .unwrap();
    assert!(device.finish().is_err());
    let mut device = TextDevice::new(Limits {
        text_items: 1,
        ..Limits::default()
    });
    let font = font(1);
    device.select_font(&font).unwrap();
    show(&mut device, &font, &[glyph("a")]);
    let error = device
        .glyphs(&GlyphRun {
            font: &font,
            glyphs: &[glyph("b")],
            mode: TextRenderingMode::Fill,
            paint: GlyphPaint {
                fill: None,
                fill_state: None,
                stroke: None,
                stroke_state: None,
            },
            hidden: false,
        })
        .unwrap_err();
    assert_eq!(error.kind, pagura_core::ErrorKind::Limit);
}

fn show_error(device: &mut TextDevice) {
    let font = font(7);
    let error = device
        .glyphs(&GlyphRun {
            font: &font,
            glyphs: &[glyph("a")],
            mode: TextRenderingMode::Fill,
            paint: GlyphPaint {
                fill: None,
                fill_state: None,
                stroke: None,
                stroke_state: None,
            },
            hidden: false,
        })
        .unwrap_err();
    assert_eq!(error.message, "missing extraction font");
    assert!(device.marked(&Marked::End).is_err());
}
