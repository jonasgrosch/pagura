//! A scene pays for held tiled source identity, never byte equality or Store.

use super::*;
use crate::model::{Image, ImageMask};
use pagura_core::{ErrorKind, ObjectId};
use pagura_document::{Document, OpenOptions};
use pagura_image::ImageSource;
use pagura_testkit::fixture;

fn image_source(limits: &Limits) -> ImageSource {
    let document = Document::open(
        fixture("packed_mask_tiles", "small-flate.pdf"),
        &OpenOptions {
            limits: *limits,
            ..OpenOptions::default()
        },
    )
    .unwrap();
    let object = document.object(ObjectId::new(6, 0)).unwrap();
    ImageSource::from_xobject_with_tiling(&document, object).unwrap()
}

fn image(source: Arc<ImageSource>) -> Arc<Image> {
    Arc::new(Image {
        interpolate: false,
        source,
        space: Some(Arc::new(ColorSpace::DeviceGray)),
        decode: Arc::from([[0.0, 1.0]]),
        intent: None,
        mask: ImageMask::None,
    })
}

fn tiled() -> Arc<Image> {
    let source = image_source(&Limits {
        pixels: 10,
        ..Limits::default()
    });
    assert!(source.is_tiled_mask());
    image(Arc::new(source))
}

fn mark<'a>(image: &'a Arc<Image>, composite: &'a Arc<Composite>) -> ImageMark<'a> {
    ImageMark {
        image,
        transform: Transform::IDENTITY,
        stencil: None,
        state: state(composite),
    }
}

fn recorded(images: &[Arc<Image>]) -> Scene {
    let mut recorder = SceneRecorder::new(Limits::default());
    recorder.begin_page(&page()).unwrap();
    let composite = Arc::new(Composite::default());
    for image in images {
        recorder.image(&mark(image, &composite)).unwrap();
    }
    recorder.end_page().unwrap();
    recorder.finish().unwrap()
}

#[test]
fn forty_shared_tiled_roots_pay_once_and_distinct_equal_roots_pay_separately() {
    let shared = tiled();
    let reused = vec![Arc::clone(&shared); 40];
    let distinct: Vec<_> = (0..40)
        .map(|_| image(Arc::new(shared.source.as_ref().clone())))
        .collect();
    let one_scene = recorded(&reused);
    let distinct_scene = recorded(&distinct);
    assert_eq!(one_scene.nodes.len(), 40);
    assert_eq!(distinct_scene.nodes.len(), 40);
    let source = 3 * owned::ARC + shared.source.retained_bytes();
    // Every node/model/Decode/space charge is still per occurrence. Only the
    // proven source-owner alias differs; recording indexes are absent here.
    assert_eq!(distinct_scene.bytes - one_scene.bytes, 39 * source);
    let second_scene = recorded(&reused);
    assert_eq!(second_scene.bytes, one_scene.bytes);
    drop(one_scene);
    drop(second_scene);
    assert_eq!(recorded(&reused).bytes, distinct_scene.bytes - 39 * source);
}

#[test]
fn whole_roots_preserve_per_reference_charges_and_tiled_index_is_temporary() {
    let whole = image(Arc::new(image_source(&Limits::default())));
    assert!(!whole.source.is_tiled_mask());
    let shared = vec![Arc::clone(&whole); 40];
    let distinct: Vec<_> = (0..40)
        .map(|_| image(Arc::new(whole.source.as_ref().clone())))
        .collect();
    assert_eq!(recorded(&shared).bytes, recorded(&distinct).bytes);
    let mut sources = owned::TiledSources::default();
    let mut bytes = Meter::new(usize::MAX, "test ownership budget");
    owned::image(&whole, &mut sources, &mut bytes).unwrap();
    assert_eq!(sources.index_bytes(&bytes).unwrap(), 0);
    let shared = tiled();
    owned::image(&shared, &mut sources, &mut bytes).unwrap();
    assert!(sources.index_bytes(&bytes).unwrap() > 0);
}

#[test]
fn failed_recordings_drop_held_sources_and_do_not_discount_the_next_recording() {
    let original = tiled();
    let expected = recorded(&[Arc::clone(&original)]).bytes;
    for limit in [0, 1, expected - 1] {
        let source = Arc::new(original.source.as_ref().clone());
        let weak = Arc::downgrade(&source);
        let image = image(source);
        let mut recorder = SceneRecorder::new(Limits {
            render_bytes: limit,
            ..Limits::default()
        });
        let result = recorder
            .begin_page(&page())
            .and_then(|()| recorder.image(&mark(&image, &Arc::new(Composite::default()))));
        assert_eq!(result.unwrap_err().kind, ErrorKind::Limit);
        drop(image);
        drop(recorder);
        assert!(weak.upgrade().is_none());
        assert_eq!(recorded(&[Arc::clone(&original)]).bytes, expected);
    }
    // A finished scene, rather than its dropped identity index, owns the root.
    let source = Arc::new(original.source.as_ref().clone());
    let weak = Arc::downgrade(&source);
    let image = image(source);
    let scene = recorded(&[Arc::clone(&image)]);
    drop(image);
    assert!(weak.upgrade().is_some());
    drop(scene);
    assert!(weak.upgrade().is_none());
}
