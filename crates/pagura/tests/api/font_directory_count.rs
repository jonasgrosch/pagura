//! Explicit Repair rendering; legacy Strict-first scan cannot discover this repair.
use crate::support::ppm;
use pagura::{
    Document, ErrorKind, ObjectId, OpenOptions, Position, Recovery, RenderOptions, Rgba8,
};
use pagura_testkit::{assert_bytes_eq, fixture};

#[test]
fn explicit_repair_preserves_full_rgb_rgba_source_and_text() {
    for kind in ["ttf", "cff", "cff-cid"] {
        for count in ["valid", "count-plus-one"] {
            let name = format!("{kind}-{count}");
            let bytes = fixture("font_directory", &format!("{name}.pdf"));
            let document = Document::open(
                bytes.clone(),
                &OpenOptions {
                    recovery: Recovery::Repair,
                    ..OpenOptions::default()
                },
            )
            .unwrap();
            let page = document.page(0).unwrap();
            for dpi in [72, 73, 144] {
                let options = RenderOptions::from_dpi(f64::from(dpi));
                let image = pagura::render_page(&document, page, &options).unwrap();
                assert_bytes_eq(
                    &ppm(&image),
                    &fixture("font_directory", &format!("literal-{dpi}.ppm")),
                    &name,
                );
                assert_bytes_eq(
                    image.data(),
                    &fixture("font_directory", &format!("literal-{dpi}.white.rgba")),
                    &name,
                );
                let transparent = pagura::render_page(
                    &document,
                    page,
                    &RenderOptions {
                        background: Rgba8::TRANSPARENT,
                        ..options
                    },
                )
                .unwrap();
                assert_bytes_eq(
                    transparent.data(),
                    &fixture("font_directory", &format!("literal-{dpi}.transparent.rgba")),
                    &name,
                );
            }
            let text = pagura::text_page(&document, page).unwrap();
            let glyphs = text.glyphs();
            assert_eq!(glyphs.len(), 2, "{name}");
            for (glyph, (x, y)) in glyphs.iter().zip([(8.0, 12.0), (32.0, 9.0)]) {
                assert_eq!(glyph.unicode.as_deref(), Some("A"));
                assert_eq!((glyph.origin.x, glyph.origin.y), (x, y));
                assert_eq!((glyph.advance_end.x, glyph.advance_end.y), (x + 60.0, y));
            }
            let repairs: Vec<_> = document
                .diagnostics()
                .iter()
                .filter_map(pagura::Diagnostic::repair)
                .collect();
            assert_eq!(
                repairs,
                if count == "valid" {
                    vec![]
                } else {
                    vec![pagura::FONT_SFNT_DIRECTORY_COUNT]
                }
            );
            assert_eq!(document.original_bytes(), bytes.as_slice());
        }
    }
}

#[test]
fn strict_error_and_negative_full_page_refusals_are_preserved() {
    for kind in ["ttf", "cff", "cff-cid"] {
        let document = Document::open(
            fixture("font_directory", &format!("{kind}-count-plus-one.pdf")),
            &OpenOptions {
                recovery: Recovery::Strict,
                ..OpenOptions::default()
            },
        )
        .unwrap();
        let error = pagura::render_page(
            &document,
            document.page(0).unwrap(),
            &RenderOptions::default(),
        )
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax);
        assert_eq!(error.message, "sfnt table overlaps directory");
        // Only an available substitute provider adds this existing Strict tag.
        // Without bundled data the original program error remains untagged.
        assert_eq!(
            error.repair,
            pagura::provider::BUNDLED.then_some(pagura_text::repairs::EMBEDDED_UNUSABLE)
        );
        let object = ObjectId::new(7, 0);
        assert_eq!(error.object, Some(object));
        assert_eq!(
            error.position,
            Position::Stream {
                object,
                offset: if kind == "ttf" { 52 } else { 20 }
            }
        );
        if kind == "cff" {
            continue;
        }
        for fault in [
            "count-plus-two",
            "first-offset-inside-record",
            "duplicate-prefix-tag",
            "wrong-first-payload-header",
            "prefix-out-of-bounds",
            "unknown-signature",
            "truncated-extra-header",
        ] {
            for recovery in [Recovery::Strict, Recovery::Repair] {
                let document = Document::open(
                    fixture("font_directory", &format!("{kind}-{fault}.pdf")),
                    &OpenOptions {
                        recovery,
                        ..OpenOptions::default()
                    },
                )
                .unwrap();
                let error = pagura::render_page(
                    &document,
                    document.page(0).unwrap(),
                    &RenderOptions::default(),
                )
                .unwrap_err();
                assert_eq!(error.kind, ErrorKind::Syntax, "{kind}/{fault}/{recovery:?}");
                assert!(
                    document
                        .diagnostics()
                        .iter()
                        .all(|d| d.repair() != Some(pagura::FONT_SFNT_DIRECTORY_COUNT))
                );
            }
        }
    }
}

#[test]
fn additional_ambiguity_and_outer_ttc_keep_full_pdf_errors() {
    use pagura_testkit::{Json, Manifest, fixture_text};
    let mut checked = 0;
    for case in Manifest::load("font_directory").cases() {
        if case.string("expected_repair_render") != Some("refused") {
            continue;
        }
        let name = case.name();
        let contract = Json::parse(&fixture_text(
            "font_directory",
            &format!("{name}.contract.json"),
        ))
        .unwrap();
        for (mode, recovery) in [("strict", Recovery::Strict), ("repair", Recovery::Repair)] {
            let expected = contract
                .get("portable_contracts")
                .unwrap()
                .get(mode)
                .unwrap()
                .get("render")
                .unwrap();
            let detail = expected.get("error").unwrap();
            let document = Document::open(
                fixture("font_directory", &format!("{name}.pdf")),
                &OpenOptions {
                    recovery,
                    ..OpenOptions::default()
                },
            )
            .unwrap();
            let error = pagura::render_page(
                &document,
                document.page(0).unwrap(),
                &RenderOptions::default(),
            )
            .unwrap_err();
            assert_eq!(error.kind, ErrorKind::Syntax, "{name}/{mode}");
            assert_eq!(Some(error.message), detail.get("message").unwrap().as_str());
            let object = ObjectId::new(7, 0);
            assert_eq!(error.object, Some(object));
            assert_eq!(
                error.position,
                Position::Stream {
                    object,
                    offset: usize::try_from(detail.get("position").unwrap().as_u64().unwrap())
                        .unwrap()
                }
            );
            let mut expected_repair = detail.get("repair").unwrap().as_str();
            if !pagura::provider::BUNDLED
                && expected_repair == Some(pagura_text::repairs::EMBEDDED_UNUSABLE.as_str())
            {
                let original = contract
                    .get("portable_contracts")
                    .unwrap()
                    .get(mode)
                    .unwrap()
                    .get("font")
                    .unwrap()
                    .get("error")
                    .unwrap();
                assert_eq!(original.get("repair").unwrap().as_str(), None);
                expected_repair = None;
            }
            assert_eq!(error.repair.map(pagura::Repair::as_str), expected_repair);
            let notes = document.diagnostics();
            let tags: Vec<_> = notes.iter().map(pagura::Diagnostic::tag).collect();
            let expected_tags: Vec<_> = expected
                .get("diagnostics")
                .unwrap()
                .as_array()
                .unwrap()
                .iter()
                .map(|j| j.as_str().unwrap())
                .collect();
            assert_eq!(tags, expected_tags, "{name}/{mode}");
            checked += 1;
        }
    }
    assert_eq!(checked, 56);
}
