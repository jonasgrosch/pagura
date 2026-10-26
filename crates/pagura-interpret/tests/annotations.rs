#![forbid(unsafe_code)]
//! Static annotation placement uses the ordinary form interpreter, skips
//! irrelevant objects and retains existing reference and scene budgets.

use pagura_core::{ErrorKind, Limits, Recovery, Result, Store};
use pagura_document::{Document, OpenOptions};
use pagura_interpret::{Interpreter, SceneRecorder};

fn open(name: &str, limits: Limits) -> Result<Document> {
    Document::open(
        pagura_testkit::fixture("annotation_display", &format!("{name}.pdf")),
        &OpenOptions {
            limits,
            recovery: Recovery::Strict,
            ..OpenOptions::default()
        },
    )
}

#[test]
fn absent_and_null_fixed_print_keep_ordinary_appearance_placement() {
    let mut expected = None;
    for name in ["watermark-ordinary-72", "fixed-null", "fixed-indirect-null"] {
        let document = open(name, Limits::default()).unwrap();
        let scene = Interpreter::new(&document)
            .scene(document.page(0).unwrap())
            .unwrap();
        assert!(scene.groups.iter().all(|group| group.view.is_none()));
        let dump = pagura_interpret::dump(&scene);
        assert_eq!(
            *expected.get_or_insert_with(|| dump.clone()),
            dump,
            "{name}"
        );
    }
}

#[test]
fn media_placement_keeps_one_existing_fixed_view_and_a_plain_form() {
    let document = open("fixed-origin-rotate-90-72", Limits::default()).unwrap();
    let scene = Interpreter::new(&document)
        .scene(document.page(0).unwrap())
        .unwrap();
    let views: Vec<_> = scene.groups.iter().filter_map(|group| group.view).collect();
    // Media [10,-6,42,18] turns to 24x32. H=.5,V=.25 gives the
    // 8x4 target [12,8,20,12]. Its upper-left corner is (30,6) in
    // default user space: x = 42-12, y = -6+12.
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].anchor, pagura_raster::Point::new(30.0, 6.0));
    assert!(views[0].no_rotate);
    assert!(!views[0].no_zoom);
    assert_eq!(scene.marks(), 2);
}

#[test]
fn static_paint_does_not_follow_ignored_keys_or_sound_payloads() {
    for name in [
        "fixed-unknown-entry",
        "sound-ignored-fixed-print",
        "sound-inert-payload",
        "sound-generated-inert-payload",
        "sound-generated-ignored-fixed-print",
        "sound-stored-ignores-visual",
    ] {
        let document = open(name, Limits::default()).unwrap();
        let scene = Interpreter::new(&document)
            .scene(document.page(0).unwrap())
            .unwrap();
        assert_eq!(scene.marks(), 2, "{name}");
        assert_eq!(document.diagnostics(), [], "{name}");
    }
    for name in [
        "fixed-hidden-malformed",
        "sound-hidden-malformed",
        "sound-no-view",
    ] {
        let document = open(name, Limits::default()).unwrap();
        assert_eq!(
            Interpreter::new(&document)
                .scene(document.page(0).unwrap())
                .unwrap()
                .marks(),
            0,
            "{name}"
        );
    }
}

#[test]
fn malformed_placement_is_read_only_when_a_device_needs_appearances() {
    for name in [
        "fixed-not-dictionary",
        "fixed-missing-type",
        "fixed-wrong-type",
        "fixed-matrix-count",
        "fixed-matrix-number",
        "fixed-horizontal-type",
        "fixed-vertical-type",
        "sound-wrong-name",
        "sound-wrong-color",
        "sound-wrong-alpha",
        "redact-invalid-type",
        "redact-invalid-empty",
        "redact-invalid-short",
        "redact-invalid-number",
        "redact-invalid-clockwise",
        "redact-invalid-concave",
        "redact-invalid-alpha",
    ] {
        let document = open(name, Limits::default()).unwrap();
        let page = document.page(0).unwrap();
        let interpreter = Interpreter::new(&document);
        assert!(interpreter.text(page).unwrap().glyphs.is_empty(), "{name}");
        let error = interpreter.scene(page).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax, "{name}: {error}");
        assert!(error.object.is_some(), "{name}: {error}");
    }
}

#[test]
fn static_appearances_use_existing_reference_and_scene_budgets() {
    for name in [
        "fixed-reference-limit",
        "sound-name-reference-limit",
        "redact-limit-reference",
    ] {
        let document = open(
            name,
            Limits {
                depth: 8,
                ..Limits::default()
            },
        )
        .unwrap();
        let page = document.page(0).unwrap();
        let interpreter = Interpreter::new(&document);
        assert!(interpreter.text(page).unwrap().glyphs.is_empty());
        let error = interpreter.scene(page).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit, "{name}");
        assert_eq!(error.message, "reference chain too long", "{name}");
    }

    for name in [
        "fixed-user-unit-144",
        "sound-speaker-72-0-generated",
        "sound-mic-72-0-generated",
        "redact-quad-72-generated",
    ] {
        let document = open(name, Limits::default()).unwrap();
        let page = document.page(0).unwrap();
        let record = |store: &Store, bytes| {
            let mut recorder = SceneRecorder::new(Limits {
                render_bytes: bytes,
                ..document.limits()
            });
            Interpreter::new(&document)
                .with_store(store)
                .run(page, &mut recorder)?;
            recorder.finish()
        };
        let store = Store::new(1 << 20);
        let first = record(&store, document.limits().render_bytes).unwrap();
        assert_eq!(
            pagura_interpret::dump(&first),
            pagura_interpret::dump(&record(&store, document.limits().render_bytes).unwrap()),
            "{name}"
        );
        let tight = first.bytes - 1;
        let error = record(&store, tight).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit, "{name}");
        assert_eq!(error.message, "scene byte budget exceeded", "{name}");
        assert_eq!(record(&Store::new(0), tight).unwrap_err(), error, "{name}");
    }
}

#[test]
fn redact_preview_preserves_source_marks_and_extracted_content() {
    for name in ["redact-text-generated", "redact-text-stored"] {
        let document = open(name, Limits::default()).unwrap();
        let original = document.original_bytes().to_vec();
        let page = document.page(0).unwrap();
        let interpreter = Interpreter::new(&document);
        let before = interpreter.text(page).unwrap();
        assert_eq!(before.glyphs.len(), 1, "{name}");
        assert_eq!(before.glyphs[0].unicode.as_deref(), Some("R"), "{name}");
        let scene = interpreter.scene(page).unwrap();
        assert!(scene.marks() >= 3, "{name}");
        let after = interpreter.text(page).unwrap();
        assert_eq!(after.glyphs.len(), 1, "{name}");
        assert_eq!(after.glyphs[0].unicode, before.glyphs[0].unicode, "{name}");
        assert_eq!(after.glyphs[0].quad, before.glyphs[0].quad, "{name}");
        assert_eq!(document.original_bytes(), original, "{name}");
        assert_eq!(document.diagnostics(), [], "{name}");
    }
    for name in [
        "redact-ignored-replacement",
        "redact-stored-ignores-invalid",
    ] {
        let document = open(name, Limits::default()).unwrap();
        let scene = Interpreter::new(&document)
            .scene(document.page(0).unwrap())
            .unwrap();
        assert_eq!(scene.marks(), 3, "{name}");
        assert_eq!(document.diagnostics(), [], "{name}");
    }
}

#[test]
fn sound_uses_explicit_view_flags_while_text_keeps_its_fixed_icon_view() {
    for ending in ["stored", "generated"] {
        for flags in [4, 12, 20, 28] {
            let name = if flags == 4 {
                format!("sound-speaker-144-90-{ending}")
            } else {
                format!("sound-explicit-view-{flags}-{ending}")
            };
            let document = open(&name, Limits::default()).unwrap();
            let scene = Interpreter::new(&document)
                .scene(document.page(0).unwrap())
                .unwrap();
            let views: Vec<_> = scene.groups.iter().filter_map(|group| group.view).collect();
            if flags == 4 {
                assert!(views.is_empty(), "{name}");
            } else {
                assert_eq!(views.len(), 1, "{name}");
                assert_eq!(
                    views[0].anchor,
                    pagura_raster::Point::new(8.0, 24.0),
                    "{name}"
                );
                assert_eq!(views[0].no_zoom, flags & 8 != 0, "{name}");
                assert_eq!(views[0].no_rotate, flags & 16 != 0, "{name}");
            }
        }
    }
    for name in ["text-view-144-90-4-missing", "text-view-144-90-4-stored"] {
        let document = open(name, Limits::default()).unwrap();
        let scene = Interpreter::new(&document)
            .scene(document.page(0).unwrap())
            .unwrap();
        let views: Vec<_> = scene.groups.iter().filter_map(|group| group.view).collect();
        assert_eq!(views.len(), 1, "{name}");
        assert!(views[0].no_zoom && views[0].no_rotate, "{name}");
    }
}
