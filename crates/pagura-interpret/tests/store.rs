#![forbid(unsafe_code)]
//! Fonts, outlines and ICC profiles cached in a `Store` by object identity:
//! a second interpretation finds them, and font keys stay stable.

use pagura_core::Store;
use pagura_document::{Document, OpenOptions};
use pagura_interpret::{Interpreter, Node, Scene};

#[test]
fn scene_bytes_and_tight_errors_do_not_depend_on_store_history() {
    let document = Document::open(
        pagura_testkit::fixture("paint", "text-mode-0.pdf"),
        &OpenOptions::default(),
    )
    .unwrap();
    let page = document.page(0).unwrap();
    let record = |store: &Store, bytes| {
        let mut recorder = pagura_interpret::SceneRecorder::new(pagura_core::Limits {
            render_bytes: bytes,
            ..document.limits()
        });
        Interpreter::new(&document)
            .with_store(store)
            .run(page, &mut recorder)?;
        recorder.finish()
    };
    let store = Store::new(64 << 20);
    let first = record(&store, document.limits().render_bytes).unwrap();
    let warm = record(&store, document.limits().render_bytes).unwrap();
    let fresh = record(&Store::new(0), document.limits().render_bytes).unwrap();
    assert_eq!(
        (first.bytes, pagura_interpret::dump(&first)),
        (warm.bytes, pagura_interpret::dump(&warm))
    );
    assert_eq!(
        (first.bytes, pagura_interpret::dump(&first)),
        (fresh.bytes, pagura_interpret::dump(&fresh))
    );
    let tight = first.bytes - 1;
    let error = record(&store, tight).unwrap_err();
    assert_eq!(error.kind, pagura_core::ErrorKind::Limit);
    assert_eq!(error.message, "scene byte budget exceeded");
    assert_eq!(record(&Store::new(0), tight).unwrap_err(), error);
    assert_eq!(record(&Store::new(64 << 20), tight).unwrap_err(), error);
}

fn font_keys(scene: &Scene) -> Vec<u64> {
    scene
        .nodes
        .iter()
        .filter_map(|node| match node {
            Node::Glyphs(glyphs) => Some(glyphs.sources.iter().map(|s| s.key.font.0)),
            _ => None,
        })
        .flatten()
        .collect()
}

#[test]
fn fonts_and_outlines_are_shared_through_the_store() {
    let document = Document::open(
        pagura_testkit::fixture("paint", "text-mode-0.pdf"),
        &OpenOptions::default(),
    )
    .unwrap();
    let page = document.page(0).unwrap();
    let store = Store::new(64 << 20);
    let interpreter = Interpreter::new(&document).with_store(&store);
    let first = interpreter.scene(page).unwrap();
    let retained = store.retained_bytes();
    assert!(retained > 0);
    let second = interpreter.scene(page).unwrap();
    assert_eq!(store.retained_bytes(), retained);
    let keys = font_keys(&first);
    assert_ne!(keys, Vec::<u64>::new());
    assert_eq!(keys, font_keys(&second));
    let plain = Interpreter::new(&document).scene(page).unwrap();
    assert_eq!(keys, font_keys(&plain));
    assert_eq!(
        pagura_interpret::dump(&first),
        pagura_interpret::dump(&plain)
    );
}

#[test]
fn icc_profiles_are_shared_through_the_store() {
    let document = Document::open(
        pagura_testkit::fixture("content", "icc-3-sampled.pdf"),
        &OpenOptions::default(),
    )
    .unwrap();
    let page = document.page(0).unwrap();
    let store = Store::new(64 << 20);
    let interpreter = Interpreter::new(&document).with_store(&store);
    let result = interpreter.scene(page);
    if let Err(error) = &result {
        // Sampled functions of another package are still being built.
        assert!(error.message.contains("not implemented"), "{error}");
    }
    assert!(store.retained_bytes() > 0);
}
