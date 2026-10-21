//! Empty Type 3 bounds recovery must preserve painting and positioned text.
#![forbid(unsafe_code)]
#![expect(
    clippy::unwrap_used,
    reason = "test failures include the rejected result"
)]
use pagura_core::{ErrorKind, ObjectId, Position, Recovery};
use pagura_document::{Document, OpenOptions};
use pagura_legacy::{Accuracy, RenderOptions, Tile, compile_page, compile_page_with_text};
use pagura_testkit::fixture;

fn document(name: &str, recovery: Recovery) -> Document {
    Document::open(
        fixture("recovery", &format!("type3-empty-bounds-{name}.pdf")),
        &OpenOptions {
            recovery,
            ..OpenOptions::default()
        },
    )
    .unwrap()
}

#[test]
fn empty_bounds_recovery_preserves_advance_paint_text_and_tiles() {
    for name in ["x", "y", "xy", "zero"] {
        let document = document(name, Recovery::Repair);
        let page = document.page(0).unwrap();
        let (list, text) = compile_page_with_text(&document, page, None).unwrap();
        assert_eq!(text.text(), "AAB", "{name}");
        for (glyph, x) in text.glyphs().iter().zip([2.0, 6.0, 10.0]) {
            assert_eq!((glyph.origin.x, glyph.origin.y), (x, 2.0), "{name}");
            assert_eq!(glyph.advance_end.x, x + 4.0, "{name}");
        }
        assert_eq!(text.glyphs().len(), 3);
        for accuracy in [Accuracy::Exact, Accuracy::Competitive] {
            let options = RenderOptions {
                accuracy,
                ..RenderOptions::default()
            };
            let full = list.render(&options).unwrap();
            assert_eq!(
                pagura_testkit::ppm(
                    full.width(),
                    full.height(),
                    &full.to_rgb_over(pagura_raster::Rgba8::WHITE),
                ),
                fixture("recovery", &format!("type3-empty-bounds-{name}.ppm")),
                "{name}: {accuracy:?}"
            );
            let tile = Tile {
                x: 9,
                y: 6,
                width: 6,
                height: 9,
            };
            let part = list.render_tile(&options, tile).unwrap();
            for row in 0..tile.height as usize {
                let start = ((tile.y as usize + row) * full.width() as usize + tile.x as usize) * 4;
                let len = tile.width as usize * 4;
                assert_eq!(
                    &part.data()[row * len..(row + 1) * len],
                    &full.data()[start..start + len]
                );
            }
        }
        let diagnostics = document.diagnostics();
        assert_eq!(diagnostics.len(), usize::from(name != "zero"), "{name}");
        if let Some(diagnostic) = diagnostics.first() {
            assert_eq!(
                diagnostic.repair(),
                Some(pagura_text::repairs::TYPE3_EMPTY_BOUNDS)
            );
            assert_eq!(diagnostic.object, Some(ObjectId::new(6, 0)));
            assert!(
                matches!(diagnostic.position, Position::Stream { object, .. } if object == ObjectId::new(6, 0))
            );
        }
        // Repeated glyphs and separate text/render requests share one diagnostic.
        compile_page(&document, page).unwrap();
        compile_page_with_text(&document, page, None).unwrap();
        assert_eq!(document.diagnostics(), diagnostics);
    }
}

#[test]
fn strict_rejects_reversed_empty_bounds_with_the_original_context() {
    for name in ["x", "y", "xy"] {
        let document = document(name, Recovery::Strict);
        let error = compile_page(&document, document.page(0).unwrap()).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax);
        assert_eq!(error.repair, Some(pagura_text::repairs::TYPE3_EMPTY_BOUNDS));
        assert_eq!(error.object, Some(ObjectId::new(6, 0)));
        assert!(
            matches!(error.position, Position::Stream { object, .. } if object == ObjectId::new(6, 0))
        );
        assert_eq!(
            document.diagnostics(),
            Vec::<pagura_core::Diagnostic>::new()
        );
    }
    let document = document("zero", Recovery::Strict);
    compile_page(&document, document.page(0).unwrap()).unwrap();
    assert_eq!(
        document.diagnostics(),
        Vec::<pagura_core::Diagnostic>::new()
    );
}

#[test]
fn recovery_does_not_hide_painting_or_malformed_procedures() {
    for recovery in [Recovery::Strict, Recovery::Repair] {
        for name in ["painted", "nested", "duplicate", "malformed"] {
            let document = document(name, recovery);
            let error = compile_page(&document, document.page(0).unwrap()).unwrap_err();
            assert_eq!(error.kind, ErrorKind::Syntax, "{name}: {error}");
            assert_eq!(error.object, Some(ObjectId::new(6, 0)), "{name}");
            assert!(document.diagnostics().is_empty(), "{name}");
        }
    }
}

#[test]
fn recovery_keeps_procedure_work_bounded() {
    let options = OpenOptions {
        limits: pagura_core::Limits {
            operations: 5,
            ..pagura_core::Limits::default()
        },
        ..OpenOptions::default()
    };
    let document =
        Document::open(fixture("recovery", "type3-empty-bounds-xy.pdf"), &options).unwrap();
    let error = compile_page(&document, document.page(0).unwrap()).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
}
