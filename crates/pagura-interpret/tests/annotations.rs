#![forbid(unsafe_code)]
//! Static annotation placement uses the ordinary form interpreter, skips
//! irrelevant objects and retains existing reference and scene budgets.

use pagura_core::{ErrorKind, Limits, Recovery, Store};
use pagura_document::{Document, OpenOptions};
use pagura_interpret::{Interpreter, SceneRecorder};

fn open(name: &str, limits: Limits) -> Document {
    Document::open(
        pagura_testkit::fixture("annotation_display", &format!("{name}.pdf")),
        &OpenOptions {
            limits,
            recovery: Recovery::Strict,
            ..OpenOptions::default()
        },
    )
    .unwrap()
}

#[test]
fn absent_and_null_fixed_print_keep_ordinary_appearance_placement() {
    let mut expected = None;
    for name in ["watermark-ordinary-72", "fixed-null", "fixed-indirect-null"] {
        let document = open(name, Limits::default());
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
    let document = open("fixed-origin-rotate-90-72", Limits::default());
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
    ] {
        let document = open(name, Limits::default());
        let scene = Interpreter::new(&document)
            .scene(document.page(0).unwrap())
            .unwrap();
        assert_eq!(scene.marks(), 2, "{name}");
        assert!(document.diagnostics().is_empty(), "{name}");
    }
    let document = open("fixed-hidden-malformed", Limits::default());
    assert_eq!(
        Interpreter::new(&document)
            .scene(document.page(0).unwrap())
            .unwrap()
            .marks(),
        0
    );
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
    ] {
        let document = open(name, Limits::default());
        let page = document.page(0).unwrap();
        let interpreter = Interpreter::new(&document);
        assert!(interpreter.text(page).unwrap().glyphs.is_empty(), "{name}");
        let error = interpreter.scene(page).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax, "{name}: {error}");
        assert!(error.object.is_some(), "{name}: {error}");
    }
}

#[test]
fn fixed_print_uses_existing_reference_and_scene_budgets() {
    let document = open(
        "fixed-reference-limit",
        Limits {
            depth: 8,
            ..Limits::default()
        },
    );
    let page = document.page(0).unwrap();
    let interpreter = Interpreter::new(&document);
    assert!(interpreter.text(page).unwrap().glyphs.is_empty());
    let error = interpreter.scene(page).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
    assert_eq!(error.message, "reference chain too long");

    let document = open("fixed-user-unit-144", Limits::default());
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
        pagura_interpret::dump(&record(&store, document.limits().render_bytes).unwrap())
    );
    let tight = first.bytes - 1;
    let error = record(&store, tight).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
    assert_eq!(error.message, "scene byte budget exceeded");
    assert_eq!(record(&Store::new(0), tight).unwrap_err(), error);
}
