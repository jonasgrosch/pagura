//! Original-source admission and actual prepared ownership under caller retile caps.

use super::assert_within_charges;
use crate::{RenderRequest, Scratch, prepare};
use pagura_core::{ErrorKind, Limits, Store, TiledImageLimits};
use pagura_document::{Document, OpenOptions};
use pagura_interpret::Interpreter;
use pagura_testkit::fixture;

#[test]
fn caller_retile_orders_and_failed_admission_preserve_store_and_complete_pixels() {
    let original = Limits {
        pixels: 30,
        decoded_bytes: 65_536,
        decode_bytes: 65_536,
        tiled_images: TiledImageLimits {
            samples: 70,
            work: 1_000,
        },
        ..Limits::default()
    };
    let document = Document::open(
        fixture("packed_mask_tiles", "small-flate.pdf"),
        &OpenOptions {
            limits: original,
            ..OpenOptions::default()
        },
    )
    .unwrap();
    let scene = Interpreter::new(&document)
        .scene(document.page(0).unwrap())
        .unwrap();
    let request = RenderRequest::page(&scene.page, 9.0 / 72.0).unwrap();
    let expected = fixture("packed_mask_tiles", "retile-one-pixel.white.rgba");
    for capacity in [0, 4096, 64 * 1024 * 1024] {
        for order in [vec![30, 20, 10, 9, 20, 30], vec![10, 20, 30, 9, 10]] {
            let store = Store::new(capacity);
            for pixels in order {
                let limits = Limits { pixels, ..original };
                let before = store.retained_bytes();
                let result = prepare(&scene, &request, &store, &limits);
                if pixels == 9 {
                    assert_eq!(result.unwrap_err().kind, ErrorKind::Limit);
                    assert_eq!(
                        store.retained_bytes(),
                        before,
                        "refused row cannot publish parent or tile cache values"
                    );
                } else {
                    let prepared = result.unwrap();
                    let mut scratch = Scratch::default();
                    let mut rendering = prepared.with_scratch(&mut scratch).unwrap();
                    let mut output = Vec::new();
                    rendering.render(&mut output);
                    assert_eq!(output, expected, "pixelcap{pixels} capacity{capacity}");
                    assert_within_charges(&rendering, prepared.charges());
                }
                assert!(store.retained_bytes() <= capacity);
            }
        }
    }
}
