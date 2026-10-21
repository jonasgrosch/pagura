//! Explicit resource provision, facade defaults and document cache lifetime.

use crate::support::ppm;
use pagura::{Document, ErrorKind, OpenOptions, Recovery, RenderContext, RenderOptions};
use pagura_testkit::fixture;
use std::sync::Arc;

const FONTS: &[&str] = &[
    "bundled-helvetica",
    "bundled-times-bold",
    "bundled-courier-oblique",
    "bundled-descriptor-flags",
    "bundled-cjk-cmap",
    "bundled-symbol",
    "bundled-zapfdingbats",
    "bundled-symbol-differences",
    "bundled-zapfdingbats-differences",
];

#[test]
fn facade_and_context_remain_send_and_sync() {
    fn check<T: Send + Sync>() {}
    check::<Document>();
    check::<RenderContext>();
}

#[test]
fn explicit_absence_of_font_data_stays_unsupported() {
    for &name in FONTS {
        let source = Document::open(
            fixture("paint", &format!("{name}.pdf")),
            &OpenOptions::default(),
        )
        .unwrap();
        let context = pagura::render_context_with_provider(Arc::new(source), None);
        let page = context.document().page(0).unwrap();
        let error = context.compile_page(page).unwrap_err();
        assert!(
            matches!(error.kind, ErrorKind::Unsupported(_)),
            "{name}: {error}"
        );
        assert_eq!(
            error.feature().unwrap().as_str(),
            if name == "bundled-cjk-cmap" {
                "font.predefined-cmap"
            } else {
                "font.non-embedded"
            }
        );
    }
}

#[test]
fn facade_feature_selects_provider_and_reuses_document_resources() {
    for name in FONTS {
        let document = Document::open(
            fixture("paint", &format!("{name}.pdf")),
            &OpenOptions::default(),
        )
        .unwrap();
        let context = pagura::render_context(Arc::new(document));
        let document = context.document();
        let page = document.page(0).unwrap();
        let result = context.compile_page(page);
        #[cfg(not(feature = "bundled-data"))]
        assert!(matches!(
            result.unwrap_err().kind,
            ErrorKind::Unsupported(_)
        ));
        #[cfg(feature = "bundled-data")]
        {
            let list = result.unwrap_or_else(|e| panic!("{name}: {e}"));
            let bytes = context.retained_bytes();
            assert!(bytes > 0);
            let again = context.compile_page(page).unwrap();
            assert_eq!(context.retained_bytes(), bytes);
            let options = RenderOptions::default();
            let image = list.render(&options).unwrap();
            assert_eq!(image.data(), again.render(&options).unwrap().data());
            let actual = ppm(&image);
            let expected = fixture("paint", &format!("{name}.rgb"));
            assert_eq!(actual.len(), expected.len());
            assert!(
                actual
                    .iter()
                    .zip(&expected)
                    .all(|(a, b)| a.abs_diff(*b) <= 1),
                "{name}: polygon oracle"
            );
            let tile = pagura::Tile {
                x: 11,
                y: 9,
                width: 37,
                height: 41,
            };
            let part = pagura::render_tile(document, page, &options, tile).unwrap();
            crate::support::assert_tile_matches(&image, &part, tile);
            assert_eq!(
                document.diagnostics().len(),
                usize::from(*name != "bundled-cjk-cmap")
            );
        }
    }
}

#[test]
fn context_rejects_foreign_pages() {
    let a = Arc::new(
        Document::open(fixture("paint", "text-mode-0.pdf"), &OpenOptions::default()).unwrap(),
    );
    let b = Document::open(fixture("paint", "text-mode-0.pdf"), &OpenOptions::default()).unwrap();
    let context = RenderContext::new(a);
    // Page clones retain the document's shared resource identity.
    let cloned = context.document().page(0).unwrap().clone();
    context.compile_page(&cloned).unwrap();
    assert_eq!(
        context.compile_page(b.page(0).unwrap()).unwrap_err().kind,
        ErrorKind::InvalidArgument
    );
}

/// A provider written against the facade alone, sharing bundled data.
#[cfg(feature = "bundled-data")]
#[test]
fn custom_provider_matches_the_facade_default_and_loads_once() {
    use pagura::provider::{Bundled, CidToUnicode, PredefinedCMap, Provider, StandardFont};
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Count {
        data: Bundled,
        calls: AtomicUsize,
    }
    impl Provider for Count {
        fn standard_font(&self, name: &[u8]) -> pagura::Result<Option<Arc<StandardFont>>> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            self.data.standard_font(name)
        }
        fn predefined_cmap(&self, name: &[u8]) -> pagura::Result<Option<Arc<PredefinedCMap>>> {
            self.data.predefined_cmap(name)
        }
        fn cid_to_unicode(
            &self,
            registry: &[u8],
            ordering: &[u8],
        ) -> pagura::Result<Option<Arc<CidToUnicode>>> {
            self.data.cid_to_unicode(registry, ordering)
        }
    }
    let provider = Arc::new(Count {
        data: Bundled::default(),
        calls: AtomicUsize::new(0),
    });
    let source = Document::open(
        fixture("paint", "bundled-helvetica.pdf"),
        &OpenOptions::default(),
    )
    .unwrap();
    let context = pagura::render_context_with_provider(Arc::new(source), Some(provider.clone()));
    let document = context.document();
    let page = document.page(0).unwrap();
    for _ in 0..3 {
        context.compile_page(page).unwrap();
    }
    assert_eq!(provider.calls.load(Ordering::Relaxed), 1);
    let options = RenderOptions::default();
    let expected = context.render_page(page, &options).unwrap();
    let default = pagura::render_page(document, page, &options).unwrap();
    assert_eq!(default.data(), expected.data());
    let tile = pagura::Tile {
        x: 0,
        y: 0,
        width: 64,
        height: 64,
    };
    let part = pagura::render_tile(document, page, &options, tile).unwrap();
    crate::support::assert_tile_matches(&expected, &part, tile);
}

#[test]
fn fonts_and_outlines_are_shared_across_document_pages() {
    let document = Document::open(
        fixture("paint", "text-font-shared-pages.pdf"),
        &OpenOptions::default(),
    )
    .unwrap();
    let context = pagura::render_context(Arc::new(document));
    let document = context.document();
    let mut retained = None;
    for page in document.pages().unwrap() {
        let list = context.compile_page(page).unwrap();
        let bytes = context.retained_bytes();
        if let Some(previous) = retained {
            assert_eq!(bytes, previous);
        }
        retained = Some(bytes);
        let options = RenderOptions::default();
        let direct = pagura::compile_page(document, page)
            .unwrap()
            .render(&options)
            .unwrap();
        assert_eq!(list.render(&options).unwrap().data(), direct.data());
    }
    assert!(retained.unwrap() > 0);
}

#[test]
fn unusable_glyphs_report_repairs_without_losing_advances() {
    let document = Document::open(
        fixture("paint", "text-unusable-glyph-advance.pdf"),
        &OpenOptions::default(),
    )
    .unwrap();
    let page = document.page(0).unwrap().clone();
    let options = RenderOptions::default();
    let context = pagura::render_context(Arc::new(document));
    let image = context.render_page(&page, &options).unwrap();
    let expected = fixture("paint", "text-unusable-glyph-advance.rgb");
    assert_eq!(ppm(&image), expected);
    let diagnostics = context.document().diagnostics();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].tag(), "font.glyph-data");
    assert!(
        matches!(diagnostics[0].position, pagura::Position::Stream { object, .. } if object == pagura::ObjectId::new(4, 0))
    );
    context.render_page(&page, &options).unwrap();
    assert_eq!(context.document().diagnostics(), diagnostics);
    let strict = Document::open(
        fixture("paint", "text-unusable-glyph-advance.pdf"),
        &OpenOptions {
            recovery: Recovery::Strict,
            ..Default::default()
        },
    )
    .unwrap();
    let error = pagura::compile_page(&strict, strict.page(0).unwrap()).unwrap_err();
    assert_eq!(error.repair.unwrap().as_str(), "font.glyph-data");
}

#[test]
#[cfg(feature = "cjk-fonts")]
fn cjk_substitute_matches_independent_otf_oracle() {
    let document = Document::open(
        fixture("paint", "bundled-cjk-substitute.pdf"),
        &OpenOptions {
            recovery: Recovery::Strict,
            ..Default::default()
        },
    )
    .unwrap();
    let context = pagura::render_context(Arc::new(document));
    let page = context.document().page(0).unwrap();
    let options = RenderOptions::default();
    let image = context.render_page(page, &options).unwrap();
    let expected = fixture("paint", "bundled-cjk-substitute.rgb");
    let actual = ppm(&image);
    assert_eq!(actual.len(), expected.len());
    assert!(actual.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 1));
    let diagnostics = context.document().diagnostics();
    assert_eq!(
        diagnostics
            .iter()
            .map(pagura::Diagnostic::tag)
            .collect::<Vec<_>>(),
        ["font.cid-glyph-missing", "font.substituted"]
    );
    let tile = pagura::Tile {
        x: 11,
        y: 9,
        width: 37,
        height: 41,
    };
    let part = context.render_tile(page, &options, tile).unwrap();
    crate::support::assert_tile_matches(&image, &part, tile);
}

#[test]
#[cfg(not(feature = "cjk-fonts"))]
fn cjk_substitution_requires_its_opt_in_data() {
    let document = Document::open(
        fixture("paint", "bundled-cjk-substitute.pdf"),
        &OpenOptions::default(),
    )
    .unwrap();
    let context = pagura::render_context(Arc::new(document));
    let error = context
        .compile_page(context.document().page(0).unwrap())
        .unwrap_err();
    assert_eq!(
        error.feature().unwrap().as_str(),
        if cfg!(feature = "bundled-data") {
            "font.non-embedded"
        } else {
            "font.predefined-cmap"
        }
    );
}
