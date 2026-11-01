#![forbid(unsafe_code)]
//! End-to-end independent pixels and original Globals diagnostic identity.
use pagura::{
    Document, ErrorKind, JBIG2_UNUSED_AGGREGATION_TABLE, ObjectId, OpenOptions, Position, Recovery,
    RenderOptions, Rgba8,
};
use pagura_testkit::{Json, Manifest, fixture, ppm};

#[test]
fn independent_pdf_twins_keep_strict_errors_and_stream_positions() {
    let area = "recovery_jbig2_tables";
    let mut rendered = 0;
    let mut refused = 0;
    for case in Manifest::load(area).cases() {
        if case.string("kind") != Some("pdf") {
            continue;
        }
        let name = case.name();
        let repair = case.get("repair_offset").and_then(Json::as_u64);
        for recovery in [Recovery::Strict, Recovery::Repair] {
            let document = Document::open(
                fixture(area, &format!("{name}.pdf")),
                &OpenOptions {
                    recovery,
                    ..Default::default()
                },
            )
            .unwrap();
            let result = pagura::render_page(
                &document,
                document.page(0).unwrap(),
                &RenderOptions::default(),
            );
            if case.string("expected") == Some("render")
                && (repair.is_none() || recovery == Recovery::Repair)
            {
                let pixels = result.unwrap_or_else(|error| panic!("{name}: {error:?}"));
                assert_eq!(
                    ppm(
                        pixels.width(),
                        pixels.height(),
                        &pixels.to_rgb_over(Rgba8::WHITE)
                    ),
                    fixture(area, &format!("{name}.ppm")),
                    "{name}"
                );
                if let Some(offset) = repair {
                    let object = ObjectId::new(
                        u32::try_from(case.get("repair_object").and_then(Json::as_u64).unwrap())
                            .unwrap(),
                        0,
                    );
                    let diagnostics = document.diagnostics();
                    assert_eq!(diagnostics.len(), 1, "{name}");
                    assert_eq!(
                        diagnostics[0].repair(),
                        Some(JBIG2_UNUSED_AGGREGATION_TABLE),
                        "{name}"
                    );
                    assert_eq!(diagnostics[0].object, Some(object), "{name}");
                    assert_eq!(
                        diagnostics[0].position,
                        Position::Stream {
                            object,
                            offset: usize::try_from(offset).unwrap()
                        },
                        "{name}"
                    );
                } else {
                    assert!(document.diagnostics().is_empty(), "{name}");
                }
                rendered += 1;
            } else {
                let error = result.unwrap_err();
                assert_eq!(error.kind, ErrorKind::Syntax, "{name}: {error:?}");
                let first_repair = repair.or_else(|| {
                    (recovery == Recovery::Strict)
                        .then(|| case.get("first_repair_offset").and_then(Json::as_u64))
                        .flatten()
                });
                if let Some(offset) = first_repair {
                    let key = if repair.is_some() {
                        "repair_object"
                    } else {
                        "first_repair_object"
                    };
                    let object = ObjectId::new(
                        u32::try_from(case.get(key).and_then(Json::as_u64).unwrap()).unwrap(),
                        0,
                    );
                    assert_eq!(error.repair, Some(JBIG2_UNUSED_AGGREGATION_TABLE), "{name}");
                    assert_eq!(error.object, Some(object), "{name}");
                    assert_eq!(
                        error.position,
                        Position::Stream {
                            object,
                            offset: usize::try_from(offset).unwrap()
                        },
                        "{name}"
                    );
                    assert!(document.diagnostics().is_empty(), "{name}");
                }
                refused += 1;
            }
        }
    }
    assert_eq!((rendered, refused), (20, 26));
}

#[test]
fn repeated_dictionary_repairs_do_not_grow_across_context_reuse() {
    use std::sync::Arc;
    let area = "recovery_jbig2_tables";
    let manifest = Manifest::load(area);
    for name in ["repeated-main-unused", "repeated-both-unused"] {
        let case = manifest.case(name).unwrap();
        let object = ObjectId::new(
            u32::try_from(case.get("repair_object").and_then(Json::as_u64).unwrap()).unwrap(),
            0,
        );
        let position = Position::Stream {
            object,
            offset: usize::try_from(case.get("repair_offset").and_then(Json::as_u64).unwrap())
                .unwrap(),
        };
        for capacity in [0, 64 * 1024 * 1024] {
            let document = Arc::new(
                Document::open(
                    fixture(area, &format!("{name}.pdf")),
                    &OpenOptions::default(),
                )
                .unwrap(),
            );
            let context =
                pagura::RenderContext::with_store_capacity(Arc::clone(&document), None, capacity);
            let mut retained = None;
            for _ in 0..4 {
                let pixels = context
                    .render_page(document.page(0).unwrap(), &RenderOptions::default())
                    .unwrap();
                assert_eq!(
                    ppm(
                        pixels.width(),
                        pixels.height(),
                        &pixels.to_rgb_over(Rgba8::WHITE)
                    ),
                    fixture(area, &format!("{name}.ppm")),
                    "{name}"
                );
                let diagnostics = document.diagnostics();
                assert_eq!(diagnostics.len(), 1, "{name}");
                assert_eq!(
                    diagnostics[0].repair(),
                    Some(JBIG2_UNUSED_AGGREGATION_TABLE),
                    "{name}"
                );
                assert_eq!(diagnostics[0].position, position, "{name}");
                assert_eq!(diagnostics[0].object, Some(object), "{name}");
                let current = context.retained_bytes();
                assert!(current <= capacity, "{name}");
                if let Some(previous) = retained {
                    assert_eq!(previous, current, "{name}");
                }
                retained = Some(current);
            }
        }
    }
}
