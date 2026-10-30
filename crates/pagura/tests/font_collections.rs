#![forbid(unsafe_code)]
//! Independent named-face rectangles through the PDF collection binding.
use pagura::{Document, Limits, OpenOptions, Recovery, RenderContext, RenderOptions};
use pagura_testkit::{Json, Manifest, fixture};
use pagura_text::repairs::COLLECTION_FACE;
use std::sync::Arc;

#[test]
fn collections_select_named_faces_and_preserve_exact_pixels() {
    let manifest = Manifest::load("font_collections");
    let mut count = 0;
    for case in manifest.cases() {
        if case.string("expected") != Some("render") {
            continue;
        }
        count += 1;
        let name = case.name();
        let repaired = case.get("strict_valid").and_then(Json::as_bool) == Some(false);
        for recovery in [Recovery::Strict, Recovery::Repair] {
            let bytes = fixture("font_collections", &format!("{name}.pdf"));
            let document = Document::open(
                bytes,
                &OpenOptions {
                    recovery,
                    ..OpenOptions::default()
                },
            )
            .unwrap();
            let result = pagura::render_page(
                &document,
                document.page(0).unwrap(),
                &RenderOptions::default(),
            );
            if repaired && recovery == Recovery::Strict {
                let error = result.unwrap_err();
                assert_eq!(error.repair, Some(COLLECTION_FACE), "{name}: {error}");
                continue;
            }
            let pixels = result.unwrap_or_else(|error| panic!("{name}: {error}"));
            let actual = pagura_testkit::ppm(
                pixels.width(),
                pixels.height(),
                &pixels.to_rgb_over(pagura::Rgba8::WHITE),
            );
            assert_eq!(
                actual,
                fixture("font_collections", &format!("{name}.ppm")),
                "{name}: {recovery:?}"
            );
            assert_eq!(
                document
                    .diagnostics()
                    .iter()
                    .filter(|d| d.repair() == Some(COLLECTION_FACE))
                    .count(),
                usize::from(repaired),
                "{name}"
            );
        }
    }
    assert_eq!(count, 23);
}

#[test]
fn collection_name_scan_budget_precedes_scene_preparation() {
    let context = RenderContext::new(Arc::new(
        Document::open(
            fixture("font_collections", "glyf-cid-pair-second.pdf"),
            &OpenOptions {
                limits: Limits {
                    font_work: 1,
                    ..Limits::default()
                },
                ..OpenOptions::default()
            },
        )
        .unwrap(),
    ));
    let page = context.document().page(0).unwrap();
    let error = context.scene(page).unwrap_err();
    assert_eq!(error.kind, pagura::ErrorKind::Limit);
}

#[test]
fn collection_faces_preserve_pdf_outline_families_and_explicit_glyph_maps() {
    let mut rendered = 0;
    let mut refused = 0;
    for case in Manifest::load("font_collection_families").cases() {
        let name = case.name();
        for recovery in [Recovery::Strict, Recovery::Repair] {
            let document = Document::open(
                fixture("font_collection_families", &format!("{name}.pdf")),
                &OpenOptions {
                    recovery,
                    ..OpenOptions::default()
                },
            )
            .unwrap();
            let original = document
                .object(pagura::ObjectId::new(6, 0))
                .unwrap()
                .value()
                .clone();
            let result = pagura::render_page(
                &document,
                document.page(0).unwrap(),
                &RenderOptions::default(),
            );
            if case.string("expected") == Some("open") {
                let error = result.unwrap_err();
                assert_eq!(error.kind, pagura::ErrorKind::Syntax, "{name}: {error}");
                assert_eq!(
                    error.message, "font collection face disagrees with PDF font subtype",
                    "{name}"
                );
                refused += 1;
            } else if recovery == Recovery::Strict {
                let error = result.unwrap_err();
                assert_eq!(error.repair, Some(COLLECTION_FACE), "{name}: {error}");
            } else {
                let pixels = result.unwrap_or_else(|error| panic!("{name}: {error}"));
                let actual = pagura_testkit::ppm(
                    pixels.width(),
                    pixels.height(),
                    &pixels.to_rgb_over(pagura::Rgba8::WHITE),
                );
                assert_eq!(
                    actual,
                    fixture("font_collection_families", &format!("{name}.ppm")),
                    "{name}"
                );
                rendered += 1;
            }
            assert_eq!(
                document
                    .object(pagura::ObjectId::new(6, 0))
                    .unwrap()
                    .value(),
                &original
            );
        }
    }
    assert_eq!(rendered, 5);
    assert_eq!(refused, 16);
    assert_ne!(
        fixture("font_collection_families", "glyf-cid2-map0.ppm"),
        fixture("font_collection_families", "glyf-cid2-map1.ppm")
    );
}
