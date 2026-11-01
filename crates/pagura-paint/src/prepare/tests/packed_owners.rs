//! Frozen independent same-ID pixels and deferred tile publication failures.

use super::assert_within_charges;
use crate::{PreparedScene, RenderRequest, Scratch, prepare};
use pagura_core::{ErrorKind, Limits, Store, TiledImageLimits};
use pagura_document::{Document, OpenOptions};
use pagura_interpret::{ImageMask, Interpreter, Node, NodeId, Scene};
use pagura_raster::Rgba8;
use pagura_testkit::fixture;
use std::sync::Arc;

fn limits() -> Limits {
    Limits {
        pixels: 10,
        decoded_bytes: 65_536,
        decode_bytes: 65_536,
        tiled_images: TiledImageLimits {
            samples: 70,
            work: 1_000,
        },
        ..Limits::default()
    }
}

fn scene(name: &str) -> Scene {
    let document = Document::open(
        fixture("packed_mask_tiles", &format!("{name}.pdf")),
        &OpenOptions {
            limits: limits(),
            ..OpenOptions::default()
        },
    )
    .unwrap();
    Interpreter::new(&document)
        .scene(document.page(0).unwrap())
        .unwrap()
}

fn image(scene: &Scene) -> &pagura_interpret::ImageNode {
    let [Node::Image(image)] = &*scene.nodes else {
        panic!("one image mark expected")
    };
    image
}

fn independent_roots(mut scene: Scene, count: u32) -> Scene {
    let template = image(&scene).clone();
    scene.nodes = (0..count)
        .map(|_| {
            let mut node = template.clone();
            let model = Arc::make_mut(&mut node.image);
            let ImageMask::Soft { mask, .. } = &mut model.mask else {
                panic!("soft mask expected")
            };
            let mask = Arc::make_mut(mask);
            mask.source = Arc::new(mask.source.as_ref().clone());
            Node::Image(node)
        })
        .collect();
    scene.groups[scene.root.index()].children = (0..count).map(NodeId).collect();
    // This hand-built test scene conservatively retains every independent
    // root. Its recorder policy is separately tested in pagura-interpret.
    scene.bytes = scene
        .bytes
        .checked_mul(usize::try_from(count).unwrap())
        .unwrap();
    scene
}

fn rendered(prepared: &PreparedScene) -> Vec<u8> {
    let mut scratch = Scratch::default();
    let mut rendering = prepared.with_scratch(&mut scratch).unwrap();
    let mut actual = Vec::new();
    rendering.render(&mut actual);
    assert_within_charges(&rendering, prepared.charges());
    actual
}

#[test]
fn late_parent_decode_never_publishes_a_pending_tile_value() {
    let source = scene("late-parent-no-jpeg-frame");
    let request = RenderRequest::page(&source.page, 9.0 / 72.0).unwrap();
    for capacity in [0, 4096, 64 * 1024 * 1024] {
        let store = Store::new(capacity);
        for _ in 0..2 {
            let before = store.retained_bytes();
            assert_eq!(
                prepare(&source, &request, &store, &limits())
                    .unwrap_err()
                    .kind,
                ErrorKind::Syntax
            );
            assert_eq!(store.retained_bytes(), before);
        }
    }
}

#[test]
fn distinct_roots_with_same_ids_keep_different_samples_in_both_cache_levels() {
    let first = scene("small-flate");
    let mut second = scene("same-id-six-present-samples");
    // Reuse exactly one immutable red parent source. Only the mask roots
    // differ; no cross-document facade/Store behavior is assumed.
    let parent = Arc::clone(&image(&first).image.source);
    let Node::Image(node) = &mut second.nodes[0] else {
        unreachable!()
    };
    Arc::make_mut(&mut node.image).source = parent;
    let first_request = RenderRequest::page(&first.page, 9.0 / 72.0).unwrap();
    for capacity in [0, 4096, 64 * 1024 * 1024] {
        let store = Store::new(capacity);
        for (source, expected) in [
            (&first, "retile-one-pixel.white.rgba"),
            (&second, "same-id-six-present-samples-rgba.white.rgba"),
            (&first, "retile-one-pixel.white.rgba"),
        ] {
            let prepared = prepare(source, &first_request, &store, &limits()).unwrap();
            assert_eq!(rendered(&prepared), fixture("packed_mask_tiles", expected));
        }
    }
}

#[test]
fn cumulative_work_failure_keeps_new_roots_pending_until_success() {
    let first = scene("small-flate");
    let mut repeated = scene("same-id-six-present-samples");
    let parent = Arc::clone(&image(&first).image.source);
    let Node::Image(node) = &mut repeated.nodes[0] else {
        unreachable!()
    };
    Arc::make_mut(&mut node.image).source = parent;
    let repeated = independent_roots(repeated, 40);
    let request = RenderRequest::page(&first.page, 9.0 / 72.0).unwrap();
    let calibration = prepare(&repeated, &request, &Store::new(0), &limits()).unwrap();
    let required = calibration.charges().render_work();
    let required_bytes = calibration.charges().render_bytes();
    assert!(required > 1 && required_bytes > 1);
    drop(calibration);
    for capacity in [0, 4096, 64 * 1024 * 1024] {
        let store = Store::new(capacity);
        drop(prepare(&first, &request, &store, &limits()).unwrap());
        let before = store.retained_bytes();
        for low in [
            Limits {
                render_work: required - 1,
                ..limits()
            },
            Limits {
                render_bytes: required_bytes - 1,
                ..limits()
            },
        ] {
            for _ in 0..2 {
                assert_eq!(
                    prepare(&repeated, &request, &store, &low).unwrap_err().kind,
                    ErrorKind::Limit
                );
                assert_eq!(
                    store.retained_bytes(),
                    before,
                    "no pending new tile value after late failure"
                );
            }
        }
        for background in [Rgba8::WHITE, Rgba8::TRANSPARENT] {
            let request = RenderRequest {
                background,
                ..request
            };
            let prepared = prepare(&repeated, &request, &store, &limits()).unwrap();
            let suffix = if background == Rgba8::WHITE {
                "white"
            } else {
                "transparent"
            };
            assert_eq!(
                rendered(&prepared),
                fixture(
                    "packed_mask_tiles",
                    &format!("forty-six-sample-marks-rgba.{suffix}.rgba")
                )
            );
        }
        assert!(store.retained_bytes() <= capacity);
    }
}
