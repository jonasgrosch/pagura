#![forbid(unsafe_code)]
//! Only embedding labels change; complete original programs own glyph identity.
use pagura::{Document, Limits, OpenOptions, Recovery, RenderOptions};
use pagura_testkit::{Manifest, fixture};
use pagura_text::repairs::PROGRAM_BINDING;

#[test]
fn cid_binding_repairs_preserve_program_pixels_and_source_objects() {
    for case in Manifest::load("font_bindings").cases() {
        let name = case.name();
        let rejected = case.string("expected") == Some("open");
        for recovery in [Recovery::Strict, Recovery::Repair] {
            let document = Document::open(
                fixture("font_bindings", &format!("{name}.pdf")),
                &OpenOptions {
                    recovery,
                    ..OpenOptions::default()
                },
            )
            .unwrap();
            let original = document
                .object(pagura::ObjectId::new(7, 0))
                .unwrap()
                .value()
                .clone();
            let result = pagura::render_page(
                &document,
                document.page(0).unwrap(),
                &RenderOptions::default(),
            );
            if rejected {
                assert!(result.is_err(), "{name}");
                continue;
            }
            if !name.ends_with("-control") && recovery == Recovery::Strict {
                let error = result.unwrap_err();
                assert_eq!(error.repair, Some(PROGRAM_BINDING), "{name}: {error}");
                assert_eq!(error.object, Some(pagura::ObjectId::new(8, 0)));
                assert_eq!(document.diagnostics(), [] as [pagura::Diagnostic; 0]);
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
                fixture("font_bindings", &format!("{name}.ppm")),
                "{name}"
            );
            assert_eq!(
                document
                    .diagnostics()
                    .iter()
                    .filter(|d| d.repair() == Some(PROGRAM_BINDING))
                    .count(),
                usize::from(!name.ends_with("-control")),
                "{name}"
            );
            assert_eq!(
                document
                    .object(pagura::ObjectId::new(7, 0))
                    .unwrap()
                    .value(),
                &original
            );
        }
    }
}

#[test]
fn cid_binding_recovery_preserves_font_work_limit() {
    let document = Document::open(
        fixture("font_bindings", "cff-key.pdf"),
        &OpenOptions {
            limits: Limits {
                font_work: 0,
                ..Limits::default()
            },
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
    assert_eq!(error.kind, pagura::ErrorKind::Limit);
    assert!(
        document
            .diagnostics()
            .iter()
            .all(|d| d.repair() != Some(PROGRAM_BINDING))
    );
}
