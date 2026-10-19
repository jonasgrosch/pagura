//! JPEG 2000 adapter tests: precision, opacity and adapter budgets.
use super::{self as jpx_image, Declared, SMaskInData};
use crate::image::Association;
use crate::{ImageAlpha, Precision, Samples};
use pagura_core::{ErrorKind, Limits};
use pagura_imagecodec::jpx;

/// A 2×1 image whose dictionary has no /ColorSpace.
const fn empty() -> Declared {
    Declared {
        width: 2,
        height: 1,
        components: None,
        indexed: false,
    }
}

fn gray(components: Vec<jpx::JpxComponent>) -> jpx::JpxImage {
    jpx::JpxImage::new(
        2,
        1,
        components,
        jpx::JpxColor::Enumerated(jpx::EnumeratedColorSpace::GRAYSCALE),
    )
}

fn placed(image: jpx::JpxImage) -> jpx::Decoded {
    let geometry = vec![jpx::ComponentGeometry::new([1, 1], [0, 0]); image.components.len()];
    jpx::Decoded::new(image, [0, 0], geometry)
}

#[test]
fn alpha_precision_premultiplication_and_live_storage_are_preserved() {
    let component = |bits, samples| jpx::JpxComponent::new(bits, 2, 1, samples);
    let mut decoded = gray(vec![component(3, vec![0, 7]), component(9, vec![0, 511])]);
    decoded.alpha = Some(1);
    let adapted = jpx_image::adapt(
        placed(decoded.clone()),
        SMaskInData::Premultiplied,
        empty(),
        &Limits::default(),
    )
    .unwrap();
    assert_eq!(adapted.pixels.precision, Precision::Uniform(3));
    assert_eq!(
        adapted.alpha,
        Some(ImageAlpha::new(
            9,
            Samples::Words(vec![0, 511]),
            Association::Premultiplied
        ))
    );
    // Decoded planes, interleaved color, opacity and both component lists
    // must fit simultaneously; the last byte still protects real storage.
    let live = 8
        + 2
        + 4
        + 2 * (size_of::<jpx::JpxComponent>()
            + size_of::<jpx::ComponentGeometry>()
            + 2 * size_of::<&jpx::JpxComponent>()
            + 1);
    let limits = Limits {
        decoded_bytes: live - 1,
        ..Limits::default()
    };
    assert_eq!(
        jpx_image::adapt(
            placed(decoded.clone()),
            SMaskInData::Premultiplied,
            empty(),
            &limits
        )
        .unwrap_err()
        .kind,
        ErrorKind::Limit
    );
    jpx_image::adapt(
        placed(decoded),
        SMaskInData::Premultiplied,
        empty(),
        &Limits {
            decoded_bytes: live,
            ..limits
        },
    )
    .unwrap();
}

#[test]
fn unused_opacity_layout_is_ignored() {
    let mut decoded = gray(vec![
        jpx::JpxComponent::new(8, 2, 1, vec![12, 26]),
        jpx::JpxComponent::new(8, 1, 1, vec![0]),
    ]);
    decoded.alpha = Some(1);
    let adapted = jpx_image::adapt(
        placed(decoded),
        SMaskInData::Ignore,
        empty(),
        &Limits::default(),
    )
    .unwrap();
    assert_eq!(adapted.pixels.samples, Samples::Bytes(vec![12, 26]));
    assert_eq!(adapted.alpha, None);
}

#[test]
fn retained_jpx_metadata_capacity_counts_against_the_adapter_budget() {
    for geometry in [false, true] {
        let mut decoded = placed(gray(vec![jpx::JpxComponent::new(8, 2, 1, vec![12, 26])]));
        if geometry {
            decoded.component_geometry.reserve_exact(127);
        } else {
            decoded.image.components.reserve_exact(127);
        }
        let error = jpx_image::adapt(
            decoded,
            SMaskInData::Ignore,
            empty(),
            &Limits {
                decoded_bytes: 1024,
                ..Limits::default()
            },
        )
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit);
    }
}
