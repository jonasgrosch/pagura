//! Independent integer-geometry references for admitted translucent composition.
use super::*;
use crate::Accuracy;
use crate::color::{Color, Space};
use crate::display::{GlyphHulls, GlyphRun};
use crate::transparency::Attributes;
use pagura_core::{ErrorKind, ObjectId, Position, Rect};
use pagura_document::{Document, OpenOptions, Rotation};
use pagura_raster::Rgba8;

fn rectangle(bounds: [f64; 4], alpha: f64, color: [f64; 3]) -> Command {
    let [x, y, width, height] = bounds;
    let mut path = PathBuilder::new();
    path.rect(x, y, width, height);
    Command::Path(PathCommand {
        path: Arc::new(path.finish()),
        mark: Mark::Fill(FillRule::NonZero),
        clip: None,
        color: Color::Solid {
            space: Space::Rgb,
            components: color.to_vec(),
        },
        position: Position::Unknown,
        transfer: None,
        composite: Composite {
            alpha,
            ..Composite::opaque()
        },
    })
}

fn list(width: f64, height: f64, commands: Vec<Command>) -> DisplayList {
    DisplayList {
        commands,
        crop: Rect::from_corners(0.0, 0.0, width, height).unwrap(),
        rotation: Rotation::None,
        user_unit: 1.0,
        limits: Limits::default(),
        group: Attributes::default(),
        print_intent: false,
        separations: std::sync::OnceLock::new(),
    }
}

fn options(accuracy: Accuracy) -> RenderOptions {
    RenderOptions {
        accuracy,
        ..RenderOptions::default()
    }
}

#[test]
fn full_coverage_half_alpha_keeps_the_unrounded_backdrop_in_both_modes() {
    for (alpha, expected, format) in [
        (0.5, [255, 64, 64, 255], plan::Format::F32),
        (1.0, [255, 0, 0, 255], plan::Format::N16),
    ] {
        let list = list(
            1.0,
            1.0,
            (0..2)
                .map(|_| rectangle([0.0, 0.0, 1.0, 1.0], alpha, [1.0, 0.0, 0.0]))
                .collect(),
        );
        assert_eq!(
            plan::page_with_shading_patterns(&list, &Separations::default())
                .unwrap()
                .format,
            format
        );
        // Full integer coverage: green/blue are 255*(1-alpha)^2.
        // For alpha=1/2 this is exactly 63.75, whose nearest byte is 64.
        // Rounding alpha to 128/255 before the stores instead produces 63.
        for accuracy in [Accuracy::Exact, Accuracy::Competitive] {
            assert_eq!(list.render(&options(accuracy)).unwrap().data(), expected);
        }
    }
}

#[test]
fn sub_byte_source_alpha_is_not_erased_before_composition() {
    // Eight full-coverage black marks: 255*(1023/1024)^8.
    // Both integer powers and their products fit u128; no float reference,
    // renderer output, per-mark rounding or tolerance constructs this answer.
    let numerator = 255_u128 * 1023_u128.pow(8);
    let denominator = 1024_u128.pow(8);
    let expected = u8::try_from((2 * numerator + denominator) / (2 * denominator)).unwrap();
    assert_eq!(expected, 253);
    let list = list(
        1.0,
        1.0,
        (0..8)
            .map(|_| rectangle([0.0, 0.0, 1.0, 1.0], 1.0 / 1024.0, [0.0; 3]))
            .collect(),
    );
    assert_eq!(
        plan::page_with_shading_patterns(&list, &Separations::default())
            .unwrap()
            .format,
        plan::Format::F32
    );
    for accuracy in [Accuracy::Exact, Accuracy::Competitive] {
        assert_eq!(
            list.render(&options(accuracy)).unwrap().data(),
            [expected, expected, expected, 255]
        );
    }
}

#[test]
fn compiled_text_knockout_keeps_distinct_independent_answers() {
    for (knockout, level, format) in [
        (false, 64_u8, plan::Format::F32),
        (true, 128, plan::Format::F64),
    ] {
        let document = Document::open(
            pagura_testkit::fixture("paint", &format!("text-knockout-{knockout}.pdf")),
            &OpenOptions {
                recovery: pagura_core::Recovery::Strict,
                ..OpenOptions::default()
            },
        )
        .unwrap();
        let list = crate::compile_page(&document, document.page(0).unwrap()).unwrap();
        assert_eq!(
            plan::page_with_shading_patterns(&list, &Separations::default())
                .unwrap()
                .format,
            format
        );
        // The independently authored embedded outline is an 8x12 rectangle
        // at PDF (4,4); on a 32-pixel top-down page it occupies y=16..28.
        // TK false: 255*(1/2)^2 rounds 64. TK true replaces, so 255/2 rounds 128.
        for accuracy in [Accuracy::Exact, Accuracy::Competitive] {
            let output = list.render(&options(accuracy)).unwrap();
            assert_eq!((output.width(), output.height()), (32, 32));
            for y in 0_u32..32 {
                for x in 0_u32..32 {
                    let expected = if (4..12).contains(&x) && (16..28).contains(&y) {
                        [255, level, level, 255]
                    } else {
                        [255; 4]
                    };
                    let offset = usize::try_from((y * 32 + x) * 4).unwrap();
                    assert_eq!(
                        &output.data()[offset..offset + 4],
                        &expected,
                        "{accuracy:?}, TK{knockout}, ({x},{y})"
                    );
                }
            }
        }
        assert_eq!(document.diagnostics(), [] as [pagura_core::Diagnostic; 0]);
    }
}

#[test]
fn colored_backdrop_and_nonzero_tiles_share_the_translucent_recurrence() {
    let list = list(
        259.0,
        3.0,
        (0..2)
            .map(|_| rectangle([255.0, 0.0, 3.0, 2.0], 0.5, [1.0, 0.0, 0.0]))
            .collect(),
    );
    let background = Rgba8 {
        r: 200,
        g: 100,
        b: 52,
        a: 255,
    };
    // The mark straddles the 256-pixel chunk boundary. After two half-alpha
    // red marks, RGB=(255*(1-1/4)+200/4,100/4,52/4)=(241.25,25,13).
    // The unchanged top row and side columns independently check background.
    let expected = |x, y| {
        if (255..258).contains(&x) && (1..3).contains(&y) {
            [241, 25, 13, 255]
        } else {
            [200, 100, 52, 255]
        }
    };
    for accuracy in [Accuracy::Exact, Accuracy::Competitive] {
        let options = RenderOptions {
            background,
            ..options(accuracy)
        };
        let full = list.render(&options).unwrap();
        for tile in [
            Tile {
                x: 0,
                y: 0,
                width: 259,
                height: 3,
            },
            Tile {
                x: 254,
                y: 0,
                width: 5,
                height: 3,
            },
            Tile {
                x: 256,
                y: 1,
                width: 2,
                height: 2,
            },
        ] {
            let output = list.render_tile(&options, tile).unwrap();
            for y in 0..tile.height {
                for x in 0..tile.width {
                    let offset = usize::try_from((y * tile.width + x) * 4).unwrap();
                    let full_offset =
                        usize::try_from(((tile.y + y) * 259 + tile.x + x) * 4).unwrap();
                    assert_eq!(
                        &output.data()[offset..offset + 4],
                        &expected(tile.x + x, tile.y + y),
                        "{accuracy:?}, tile{tile:?}, ({x},{y})"
                    );
                    assert_eq!(
                        &output.data()[offset..offset + 4],
                        &full.data()[full_offset..full_offset + 4]
                    );
                }
            }
        }
    }
}

#[test]
fn competitive_f32_fragments_keep_offscreen_source_errors() {
    for (name, kind, feature) in [
        ("icc", "unsupported", Some("colorspace.ICCBased")),
        ("type4", "syntax", None),
    ] {
        let document = Document::open(
            pagura_testkit::fixture("content", &format!("opaque-error-{name}-visible.pdf")),
            &OpenOptions {
                recovery: pagura_core::Recovery::Strict,
                ..OpenOptions::default()
            },
        )
        .unwrap();
        let mut list = crate::compile_page(&document, document.page(0).unwrap()).unwrap();
        assert_eq!(list.commands.len(), 1);
        let Some(Command::Path(mut paint)) = list.commands.pop() else {
            panic!("fixture must contain one direct path");
        };
        paint.composite.alpha = 0.5;
        let position = paint.position;
        let mut run = GlyphRun::new(paint);
        let mut outline = PathBuilder::new();
        outline.rect(0.0, 0.0, 1.0, 1.0);
        let outline = Arc::new(outline.finish());
        let mut hulls = GlyphHulls::default();
        for x in [-4.0, 12.0] {
            run.push(
                Arc::clone(&outline),
                Transform::translate(x, 4.0),
                position,
                &mut hulls,
            )
            .unwrap();
        }
        // The aggregate intersects the page, while neither glyph intersects
        // even its one-pixel padded viewport. Competitive still converts the
        // source once before individual visibility; Exact's dense route does
        // not, and intentionally keeps its existing successful blank result.
        list.commands = vec![Command::Glyphs(Box::new(run))];
        assert!(!list.plain_opaque());
        assert_eq!(
            plan::page_with_shading_patterns(&list, &Separations::default())
                .unwrap()
                .format,
            plan::Format::F32
        );
        let error = list.render(&options(Accuracy::Competitive)).unwrap_err();
        assert_eq!(error.kind.name(), kind);
        assert_eq!(error.feature().map(pagura_core::Feature::as_str), feature);
        assert_eq!(error.object, Some(ObjectId::new(5, 0)));
        let expected_position = if name == "type4" {
            assert_eq!(error.kind, ErrorKind::Syntax);
            Position::Stream {
                object: ObjectId::new(5, 0),
                offset: 4,
            }
        } else {
            position
        };
        assert_eq!(error.position, expected_position);
        let tile = Tile {
            x: 3,
            y: 3,
            width: 1,
            height: 1,
        };
        assert_eq!(
            list.render_tile(&options(Accuracy::Competitive), tile)
                .unwrap_err(),
            error
        );
        assert_eq!(
            list.render(&options(Accuracy::Exact)).unwrap().data(),
            [255; 8 * 8 * 4]
        );
        assert_eq!(
            list.render_tile(&options(Accuracy::Exact), tile)
                .unwrap()
                .data(),
            [255; 4]
        );
        assert_eq!(document.diagnostics(), [] as [pagura_core::Diagnostic; 0]);
    }
}
