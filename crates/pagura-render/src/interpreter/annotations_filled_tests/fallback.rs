use super::*;

#[test]
fn unsupported_edited_fields_fail_while_untouched_fields_keep_stored_appearances() {
    for (kind, feature) in [
        ("type0", crate::form_appearance::features::FONT_ENCODING),
        ("hybrid-xfa", pagura_document::forms::features::XFA),
        ("oblique-tm", crate::form_appearance::features::TEXT_MATRIX),
    ] {
        for recovery in [Recovery::Strict, Recovery::Repair] {
            let name = format!("render-fallback-{kind}");
            let document = Arc::new(
                Document::open(
                    fixture("forms", &format!("{name}.pdf")),
                    &OpenOptions {
                        recovery,
                        ..Default::default()
                    },
                )
                .unwrap(),
            );
            let context = RenderContext::new(Arc::clone(&document));
            let page = document.page(0).unwrap();
            let ordinary = compile_page(&document, page)
                .unwrap()
                .render(&RenderOptions::default())
                .unwrap();
            assert_eq!(document.diagnostics(), [] as [pagura_core::Diagnostic; 0]);
            let mut filler = document.form_filler().unwrap().unwrap();
            for _ in 0..2 {
                let kept = context.compile_filled_page(&filler, page).unwrap();
                assert_eq!(
                    kept.render(&RenderOptions::default()).unwrap().data(),
                    ordinary.data()
                );
                assert_pixels(&kept, &name, 1.0);
            }
            assert_eq!(document.diagnostics().len(), 1);
            let error = match filler.set(0, FillValue::Text("AB")) {
                Err(error) => error,
                Ok(_) => context.compile_filled_page(&filler, page).unwrap_err(),
            };
            assert_eq!(error.kind, ErrorKind::Unsupported(feature), "{kind}");
            assert_eq!(document.diagnostics().len(), 1);
        }
    }
}

#[test]
fn need_appearances_does_not_turn_resource_failures_into_success() {
    let document = Document::open(
        fixture("forms", "render-text-need-appearances.pdf"),
        &OpenOptions {
            recovery: Recovery::Strict,
            limits: pagura_core::Limits {
                operations: 128,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    compile_page(&document, document.page(0).unwrap()).unwrap();
    let filler = document.form_filler().unwrap().unwrap();
    let error = compile_filled_page(&filler, document.page(0).unwrap(), None).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
    assert_eq!(document.diagnostics(), [] as [pagura_core::Diagnostic; 0]);
}
