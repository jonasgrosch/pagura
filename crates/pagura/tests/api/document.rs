//! Document structure through the public API.

use crate::support::{open, open_strict};
use pagura::form_appearance::Appearance;
use pagura::forms::FillValue;
use pagura::{Document, ErrorKind, ObjectId, OpenOptions, Position, Rect};
use pagura_testkit::fixture;

#[test]
fn independent_fixture_has_valid_object_and_page_graph() {
    let bytes = fixture("classic", "rectangle.pdf");
    let document = open(bytes.clone()).unwrap();
    assert_eq!(document.object_count(), 4);
    assert_eq!(document.page_count().unwrap(), 1);
    let page = document.page(0).unwrap();
    assert_eq!(
        page.media_box(),
        Rect::from_corners(0.0, 0.0, 64.0, 64.0).unwrap()
    );
    assert_eq!(page.crop_box(), page.media_box());
    let streams = document.content_streams(page).unwrap();
    assert_eq!(
        &*document.stream_data(streams[0]).unwrap(),
        b"1 0 0 rg 8 8 48 32 re f\n"
    );
    assert_eq!(document.original_bytes(), bytes.as_slice());
    assert_eq!(document.diagnostics(), [] as [pagura::Diagnostic; 0]);
}

#[test]
fn objects_are_cached_and_wrong_generations_do_not_resolve() {
    let document = open(fixture("classic", "rectangle.pdf")).unwrap();
    let first = document.object(ObjectId::new(1, 0)).unwrap();
    let again = document.object(ObjectId::new(1, 0)).unwrap();
    assert!(std::ptr::eq(first, again));
    let error = document.object(ObjectId::new(1, 1)).unwrap_err();
    assert_eq!(error.kind, ErrorKind::MissingObject);
    assert_eq!(error.object, Some(ObjectId::new(1, 1)));
}

#[test]
fn inheritance_normalization_and_limits() {
    let inherited = open(fixture("classic", "inherited-box.pdf")).unwrap();
    assert_eq!(inherited.page(0).unwrap().media_box().width(), 64.0);
    let normalized = open(fixture("classic", "normalized-media-box.pdf")).unwrap();
    assert_eq!(
        normalized.page(0).unwrap().media_box(),
        Rect::from_corners(0.0, 0.0, 64.0, 64.0).unwrap()
    );
    let options = OpenOptions {
        limits: pagura::Limits {
            pages: 0,
            ..pagura::Limits::default()
        },
        ..OpenOptions::default()
    };
    let limited = Document::open(fixture("classic", "rectangle.pdf"), &options).unwrap();
    assert_eq!(limited.pages().unwrap_err().kind, ErrorKind::Limit);
}

#[test]
fn invalid_rotation_is_a_syntax_error() {
    let document = open(fixture("classic", "rotate-not-multiple-of-90.pdf")).unwrap();
    assert_eq!(document.pages().unwrap_err().kind, ErrorKind::Syntax);
}

#[test]
fn trailing_bytes_are_a_reported_repair_and_a_strict_error() {
    let bytes = fixture("classic", "trailing-bytes.pdf");
    let document = open(bytes.clone()).unwrap();
    let diagnostics = document.diagnostics();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].tag(), "file.trailing-bytes");
    let error = open_strict(bytes).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Syntax);
    assert!(matches!(error.position, Position::File(_)));
}

#[test]
fn corrupt_offsets_and_truncated_tails_are_strict_errors_and_reported_repairs() {
    let bytes = fixture("classic", "rectangle.pdf");
    for length in [0, 8, bytes.len() / 2, bytes.len() - 6] {
        assert!(open_strict(bytes[..length].to_vec()).is_err(), "{length}");
    }
    for length in [0, 8] {
        assert!(open(bytes[..length].to_vec()).is_err(), "{length}");
    }
    // Missing content preserves the page tree. The undefined Contents
    // reference is null (7.3.10), so the page renders without content, and
    // both the reconstruction and the undefined reference are reported: the
    // result is never a silent partial render.
    let truncated = open(bytes[..bytes.len() / 2].to_vec()).unwrap();
    pagura::render_page(
        &truncated,
        truncated.page(0).unwrap(),
        &pagura::RenderOptions::default(),
    )
    .unwrap();
    let tags: Vec<&str> = truncated
        .diagnostics()
        .iter()
        .map(pagura::Diagnostic::tag)
        .collect();
    assert!(tags.contains(&"xref.rebuilt"), "{tags:?}");
    assert!(tags.contains(&"pages.contents-undefined"), "{tags:?}");
    let repaired = open(bytes[..bytes.len() - 6].to_vec()).unwrap();
    assert_eq!(repaired.page_count().unwrap(), 1);
    assert_eq!(repaired.diagnostics()[0].tag(), "xref.rebuilt");
    let mut damaged = bytes;
    let marker = damaged.windows(9).rposition(|w| w == b"startxref").unwrap();
    damaged[marker + 10] = b'9';
    assert!(open_strict(damaged.clone()).is_err());
    let repaired = open(damaged).unwrap();
    assert_eq!(repaired.page_count().unwrap(), 1);
    assert_eq!(repaired.diagnostics()[0].tag(), "xref.rebuilt");
}

#[test]
fn repeated_page_nodes_are_rejected() {
    let mut data = fixture("classic", "rectangle.pdf");
    let at = data.windows(12).position(|x| x == b"/Kids [3 0 R").unwrap();
    data[at + 7] = b'2';
    let document = open(data).unwrap();
    assert_eq!(document.pages().unwrap_err().kind, ErrorKind::Cycle);
}

#[test]
fn stream_length_mismatch_is_not_repaired_silently() {
    let mut data = fixture("classic", "rectangle.pdf");
    let at = data.windows(8).position(|x| x == b"/Length ").unwrap();
    data[at + 8] = b'9';
    let strict = open_strict(data.clone()).unwrap();
    let error = strict.content_streams(strict.page(0).unwrap()).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Syntax);
    assert_eq!(error.repair.unwrap().as_str(), "stream.length-corrected");
    assert_eq!(error.object, Some(ObjectId::new(4, 0)));
    let document = open(data.clone()).unwrap();
    let page = document.page(0).unwrap();
    let streams = document.content_streams(page).unwrap();
    assert_eq!(streams.len(), 1);
    assert_eq!(
        document.stream_data(streams[0]).unwrap().as_ref(),
        b"1 0 0 rg 8 8 48 32 re f\n"
    );
    assert_eq!(document.original_bytes(), data);
    let diagnostics = document.diagnostics();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].tag(), "stream.length-corrected");
    let context = pagura::render_context(std::sync::Arc::new(document));
    let pixels = context
        .render_page(
            context.document().page(0).unwrap(),
            &pagura::RenderOptions::default(),
        )
        .unwrap();
    assert_eq!(
        pagura_testkit::ppm(
            pixels.width(),
            pixels.height(),
            &pixels.to_rgb_over(pagura::Rgba8::WHITE)
        ),
        fixture("classic", "rectangle.ppm")
    );
}

#[test]
fn arbitrary_byte_changes_do_not_panic() {
    let bytes = fixture("classic", "rectangle.pdf");
    for index in (0..bytes.len()).step_by(3) {
        for byte in [0, 255, b'(', b'[', b'9'] {
            let mut data = bytes.clone();
            data[index] = byte;
            if let Ok(document) = open(data)
                && let Ok(pages) = document.pages()
            {
                for page in pages {
                    let _ = pagura::render_page(&document, page, &pagura::RenderOptions::default());
                }
            }
        }
    }
}

#[test]
fn form_preparation_hides_a_signature_without_appearance() {
    let document = open_strict(fixture("forms", "text-beside-signature.pdf")).unwrap();
    let mut filler = document.form_filler().unwrap().unwrap();
    let unchanged = pagura::prepare_form(&filler).unwrap();
    filler.set(0, FillValue::Text("AB")).unwrap();
    let filled = pagura::prepare_form(&filler).unwrap();
    for appearances in [unchanged, filled] {
        assert_eq!(appearances.len(), 2);
        assert!(matches!(
            appearances[0].appearance(),
            Appearance::Generated(_)
        ));
        assert_eq!(appearances[1].appearance(), &Appearance::Hidden);
    }
}
