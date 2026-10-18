use super::*;

#[test]
fn appearance_layout_and_painting_reuse_the_context_font_entry() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Counted {
        inner: RectangleProvider,
        calls: AtomicUsize,
    }
    impl pagura_text::Provider for Counted {
        fn standard_font(
            &self,
            base: &[u8],
        ) -> pagura_core::Result<Option<Arc<pagura_text::StandardFont>>> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            self.inner.standard_font(base)
        }
        fn predefined_cmap(
            &self,
            _: &[u8],
        ) -> pagura_core::Result<Option<Arc<pagura_text::PredefinedCMap>>> {
            Ok(None)
        }
        fn cid_to_unicode(
            &self,
            _: &[u8],
            _: &[u8],
        ) -> pagura_core::Result<Option<Arc<pagura_text::CidToUnicode>>> {
            Ok(None)
        }
    }
    let provider = Arc::new(Counted {
        inner: RectangleProvider::new("render-shared-font-cache"),
        calls: AtomicUsize::new(0),
    });
    let document = Arc::new(
        Document::open(
            fixture("forms", "render-shared-font-cache.pdf"),
            &OpenOptions::default(),
        )
        .unwrap(),
    );
    let context = RenderContext::with_provider(Arc::clone(&document), provider.clone());
    let mut filler = document.form_filler().unwrap().unwrap();
    filler.set(0, FillValue::Text("A")).unwrap();
    for _ in 0..3 {
        assert_pixels(
            &context
                .compile_filled_page(&filler, document.page(0).unwrap())
                .unwrap(),
            "render-shared-font-cache",
            1.0,
        );
    }
    // Six per-widget descriptor queries, plus one shared PDF-font load.
    // Without sharing the generation load, the provider would be called 13 times.
    assert_eq!(provider.calls.load(Ordering::Relaxed), 7);
    let embedded = Arc::new(
        Document::open(
            fixture("forms", "render-shared-embedded-font.pdf"),
            &OpenOptions::default(),
        )
        .unwrap(),
    );
    let context = RenderContext::new(Arc::clone(&embedded));
    let value = pagura_core::Value::Reference(pagura_core::ObjectId::new(70, 0));
    let first =
        crate::interpreter::text::cache::pdf_font(&embedded, &value, None, Some(&context.fonts))
            .unwrap();
    let mut filler = embedded.form_filler().unwrap().unwrap();
    filler.set(0, FillValue::Text("A")).unwrap();
    for _ in 0..3 {
        assert_pixels(
            &context
                .compile_filled_page(&filler, embedded.page(0).unwrap())
                .unwrap(),
            "render-shared-embedded-font",
            1.0,
        );
        let cached = crate::interpreter::text::cache::pdf_font(
            &embedded,
            &value,
            None,
            Some(&context.fonts),
        )
        .unwrap();
        assert!(Arc::ptr_eq(&first, &cached));
    }
}
