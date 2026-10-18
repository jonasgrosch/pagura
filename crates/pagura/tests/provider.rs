#![forbid(unsafe_code)]
//! Explicit resource provision, facade defaults and document cache lifetime.
use pagura::{Document, ErrorKind, OpenOptions, RenderContext};
use pagura_testkit::fixture;
use std::sync::Arc;

const FONTS: &[&str] = &[
    "bundled-helvetica",
    "bundled-times-bold",
    "bundled-courier-oblique",
    "bundled-descriptor-flags",
    "bundled-cjk-cmap",
];

#[test]
fn facade_and_context_remain_send_and_sync() {
    fn check<T: Send + Sync>() {}
    check::<Document>();
    check::<RenderContext>();
}

#[test]
fn explicit_absence_and_symbolic_fonts_stay_unsupported() {
    for name in FONTS
        .iter()
        .copied()
        .chain(["bundled-symbol", "bundled-zapfdingbats"])
    {
        let source = Document::open(
            fixture("paint", &format!("{name}.pdf")),
            &OpenOptions::default(),
        )
        .unwrap();
        let error = pagura_render::compile_page(&source, source.page(0).unwrap()).unwrap_err();
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
            let options = pagura::RenderOptions::default();
            let image = list.render(&options).unwrap();
            assert_eq!(image.data(), again.render(&options).unwrap().data());
            let actual = pagura_testkit::ppm(
                image.width(),
                image.height(),
                &image.to_rgb_over(pagura::Rgba8::WHITE),
            );
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
            for y in 0..tile.height {
                let start = usize::try_from(tile.x).unwrap() * 4;
                assert_eq!(
                    part.row(y).unwrap(),
                    &image.row(tile.y + y).unwrap()
                        [start..start + usize::try_from(tile.width).unwrap() * 4]
                );
            }
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

#[cfg(feature = "bundled-data")]
#[test]
fn provider_and_explicit_entry_points_agree_and_load_once() {
    use pagura_text::{CidToUnicode, PredefinedCMap, Provider, StandardFont};
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Count {
        data: pagura_data::Bundled,
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
        data: pagura_data::Bundled::default(),
        calls: AtomicUsize::new(0),
    });
    let source = Document::open(
        fixture("paint", "bundled-helvetica.pdf"),
        &OpenOptions::default(),
    )
    .unwrap();
    let context = RenderContext::with_provider(Arc::new(source), provider.clone());
    let document = context.document();
    let page = document.page(0).unwrap();
    for _ in 0..3 {
        context.compile_page(page).unwrap();
    }
    assert_eq!(provider.calls.load(Ordering::Relaxed), 1);
    let options = pagura::RenderOptions::default();
    let expected = context.render_page(page, &options).unwrap();
    let explicit =
        pagura_render::render_page_with(document, page, &options, provider.as_ref()).unwrap();
    assert_eq!(explicit.data(), expected.data());
    let tile = pagura::Tile {
        x: 0,
        y: 0,
        width: 64,
        height: 64,
    };
    assert_eq!(
        pagura_render::render_tile_with(document, page, &options, tile, provider.as_ref())
            .unwrap()
            .data(),
        expected.data()
    );
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
        let options = pagura::RenderOptions::default();
        let direct = pagura_render::compile_page(document, page)
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
    let options = pagura::RenderOptions::default();
    let context = pagura::render_context(Arc::new(document));
    let image = context.render_page(&page, &options).unwrap();
    let expected = fixture("paint", "text-unusable-glyph-advance.rgb");
    assert_eq!(
        pagura_testkit::ppm(
            image.width(),
            image.height(),
            &image.to_rgb_over(pagura::Rgba8::WHITE)
        ),
        expected
    );
    let diagnostics = context.document().diagnostics();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].repair.as_str(), "font.glyph-data");
    assert!(
        matches!(diagnostics[0].position, pagura::Position::Stream { object, .. } if object == pagura::ObjectId::new(4, 0))
    );
    context.render_page(&page, &options).unwrap();
    assert_eq!(context.document().diagnostics(), diagnostics);
    let strict = Document::open(
        fixture("paint", "text-unusable-glyph-advance.pdf"),
        &OpenOptions {
            recovery: pagura::Recovery::Strict,
            ..Default::default()
        },
    )
    .unwrap();
    let error = pagura::compile_page(&strict, strict.page(0).unwrap()).unwrap_err();
    assert_eq!(error.repair.unwrap().as_str(), "font.glyph-data");
}
