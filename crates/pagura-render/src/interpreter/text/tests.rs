//! Pixel oracles, tile parity and hostile-input admission for text and images.
use crate::{RenderOptions, compile_page};
use pagura_core::{ErrorKind, Recovery};
use pagura_document::{Document, OpenOptions};
use pagura_testkit::fixture;

fn open(name: &str) -> Document {
    Document::open(
        fixture("paint", &format!("{name}.pdf")),
        &OpenOptions {
            recovery: Recovery::Strict,
            ..Default::default()
        },
    )
    .unwrap()
}
#[test]
fn paint_cache_admission_is_atomic_and_shared_across_saved_states() {
    let document = open("text-mode-0");
    let fonts = std::sync::Mutex::new(super::cache::FontCache::new(document.limits()));
    let visibility = crate::optional::Visibility::new(&document).unwrap();
    let shared = super::super::Shared {
        provider: None,
        fonts: &fonts,
        visibility: &visibility,
    };
    let session =
        super::super::Session::new(&document, document.page(0).unwrap(), &shared, None, None);
    let mut compiler = super::super::Compiler::new(&session);
    compiler.session.retain(10).unwrap();
    compiler.save_graphics().unwrap();
    compiler.session.retain(20).unwrap();
    compiler.restore_graphics().unwrap();
    assert_eq!(session.retained(), 30);
    let error = compiler.session.retain(usize::MAX).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
    assert_eq!(session.retained(), 30);
    {
        let _held = session.hold(5).unwrap();
        assert_eq!(session.retained(), 35);
    }
    assert_eq!(session.retained(), 30);
}
#[test]
fn fonts_and_outlines_are_reused() {
    let document = open("text-tj-adjustments");
    let fonts = std::sync::Mutex::new(super::cache::FontCache::new(document.limits()));
    let visibility = crate::optional::Visibility::new(&document).unwrap();
    let shared = super::super::Shared {
        provider: None,
        fonts: &fonts,
        visibility: &visibility,
    };
    let session =
        super::super::Session::new(&document, document.page(0).unwrap(), &shared, None, None);
    super::super::Compiler::new(&session).page().unwrap();
    assert_eq!(session.paint.borrow().font_counts(), (1, 1));
}

#[test]
fn malformed_font_programs_report_their_own_stream() {
    let mut bytes = fixture("paint", "text-mode-0.pdf");
    let program_start = bytes
        .windows(11)
        .position(|b| b == b"stream\n\0\x01\0\0")
        .unwrap()
        + 7;
    // An unknown sfnt version over intact tables is a reported repair: strict
    // mode rejects it with the tag, and both modes locate it in the font
    // stream (object 7), not in the content stream that selected the font.
    bytes[program_start..program_start + 4].copy_from_slice(b"bad!");
    let font_stream = pagura_core::Position::Stream {
        object: pagura_core::ObjectId::new(7, 0),
        offset: 0,
    };
    let strict = Document::open(
        bytes.clone(),
        &OpenOptions {
            recovery: Recovery::Strict,
            ..OpenOptions::default()
        },
    )
    .unwrap();
    let error = compile_page(&strict, strict.page(0).unwrap()).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Syntax);
    assert_eq!(
        error.repair.map(pagura_core::Repair::as_str),
        Some("font.sfnt-version")
    );
    assert_eq!(error.position, font_stream);
    assert_eq!(error.object, Some(pagura_core::ObjectId::new(7, 0)));
    let repaired = Document::open(bytes, &OpenOptions::default()).unwrap();
    compile_page(&repaired, repaired.page(0).unwrap()).unwrap();
    let diagnostic = repaired
        .diagnostics()
        .into_iter()
        .find(|item| item.tag() == "font.sfnt-version")
        .unwrap();
    assert_eq!(diagnostic.position, font_stream);
    assert_eq!(diagnostic.object, Some(pagura_core::ObjectId::new(7, 0)));
}

#[test]
fn text_clip_resets_report_positions_and_preserve_accumulated_outlines() {
    for (name, offsets) in [
        ("text-clip-reset", &[10][..]),
        ("text-clip-reset-retained", &[42][..]),
        ("text-clip-reset-reentered", &[33, 57][..]),
        ("text-clip-reset-restored", &[33][..]),
        ("text-clip-reset-inherited", &[33][..]),
    ] {
        let document = open(name);
        let error = compile_page(&document, document.page(0).unwrap()).unwrap_err();
        let object = pagura_core::ObjectId::new(4, 0);
        assert_eq!(error.kind, ErrorKind::Syntax);
        assert_eq!(error.repair.unwrap().as_str(), "content.text-clip-reset");
        assert_eq!(error.object, Some(object));
        assert_eq!(
            error.position,
            pagura_core::Position::Stream {
                object,
                offset: offsets[0],
            }
        );
        assert_eq!(document.diagnostics(), []);
        let repaired = Document::open(
            fixture("paint", &format!("{name}.pdf")),
            &OpenOptions::default(),
        )
        .unwrap();
        let page = repaired.page(0).unwrap();
        let list = compile_page(&repaired, page).unwrap();
        let image = list.render(&RenderOptions::default()).unwrap();
        assert_eq!(
            pagura_testkit::ppm(
                image.width(),
                image.height(),
                &image.to_rgb_over(pagura_raster::Rgba8::WHITE)
            ),
            fixture("paint", &format!("{name}.rgb")),
            "{name}"
        );
        let diagnostics = repaired.diagnostics();
        assert_eq!(diagnostics.len(), offsets.len());
        for (diagnostic, &offset) in diagnostics.iter().zip(offsets) {
            assert_eq!(diagnostic.tag(), "content.text-clip-reset");
            assert_eq!(diagnostic.object, Some(object));
            assert_eq!(
                diagnostic.position,
                pagura_core::Position::Stream { object, offset }
            );
        }
        compile_page(&repaired, page).unwrap();
        assert_eq!(repaired.diagnostics(), diagnostics);
    }
}
