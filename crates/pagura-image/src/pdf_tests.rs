//! PDF image/filter integration oracles, calculated by the fixture generator.
use crate::{DecodedImage, ImageAlpha, ImageKind, MaskKind, Samples, jpx, jpx_image};
use pagura_core::{ErrorKind, Limits, ObjectId, Value};
use pagura_document::{Document, OpenOptions};
use pagura_testkit::fixture;
use std::collections::BTreeMap;

fn values(bytes: &[u8]) -> Vec<u16> {
    std::str::from_utf8(bytes)
        .unwrap()
        .split_whitespace()
        .map(|v| v.parse().unwrap())
        .collect()
}

pub(super) fn vector(name: &str, params: &BTreeMap<String, String>, expected: &[u8]) {
    let document = Document::open(
        fixture("image", &format!("{name}.pdf")),
        &OpenOptions::default(),
    )
    .unwrap();
    let result = crate::decode_xobject(&document, document.object(ObjectId::new(3, 0)).unwrap());
    if params["outcome"] != "valid" {
        let error = result.unwrap_err();
        match params["outcome"].as_str() {
            "syntax" => assert_eq!(error.kind, ErrorKind::Syntax, "{name}: {error}"),
            "limit" => assert_eq!(error.kind, ErrorKind::Limit, "{name}: {error}"),
            _ => assert_eq!(
                error.feature().unwrap().as_str(),
                params["feature"],
                "{name}"
            ),
        }
        assert!(error.object.is_some(), "{name}");
        return;
    }
    let image = result.unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(image.width, params["width"].parse().unwrap(), "{name}");
    assert_eq!(image.height, params["height"].parse().unwrap(), "{name}");
    assert_eq!(
        image.components,
        params["components"].parse().unwrap(),
        "{name}"
    );
    assert_eq!(
        image.bits_per_component,
        params["bits"].parse().unwrap(),
        "{name}"
    );
    assert_samples(&image.samples, &values(expected), name);
    if let Some(bits) = params.get("component_bits") {
        let bits: Vec<u8> = bits.split(',').map(|v| v.parse().unwrap()).collect();
        for (i, bits) in bits.into_iter().enumerate() {
            assert_eq!(image.component_bits(i), Some(bits), "{name}");
        }
    }
    assert_eq!(image.component_bits(usize::from(image.components)), None);
    if matches!(image.kind, ImageKind::EmbeddedColor { .. }) {
        assert!(image.embedded_color_fallback().is_some());
    }
    if let Some(space) = params.get("space") {
        assert_eq!(space_name(&image.kind), space, "{name}");
    }
    if let Some(decode) = params.get("decode") {
        let expected = if decode == "none" {
            None
        } else {
            Some(
                decode
                    .split(',')
                    .map(|v| v.parse().unwrap())
                    .collect::<Vec<f64>>(),
            )
        };
        assert_eq!(image.decode, expected, "{name}");
    }
    if params.get("mask").is_some_and(|s| s == "embedded") {
        let MaskKind::Embedded(alpha) = &image.mask else {
            panic!("{name}: missing decoded alpha");
        };
        assert_eq!(
            alpha.bits_per_component,
            params["alpha_bits"].parse().unwrap()
        );
        assert!(!alpha.premultiplied);
        assert_samples(
            &alpha.samples,
            &values(&fixture("image", &format!("{name}.alpha"))),
            name,
        );
    } else {
        assert_eq!(image.mask, MaskKind::None, "{name}");
    }
}

fn space_name(kind: &ImageKind) -> &str {
    match kind {
        ImageKind::Color {
            color_space: Some(Value::Name(name)),
        } => std::str::from_utf8(name.as_bytes()).unwrap(),
        ImageKind::Color {
            color_space: Some(Value::Array(array)),
        } => std::str::from_utf8(array[0].as_name().unwrap().as_bytes()).unwrap(),
        ImageKind::EmbeddedColor {
            color: jpx::JpxColor::Icc(_),
        } => "icc",
        ImageKind::EmbeddedColor {
            color: jpx::JpxColor::Enumerated(18),
        } => "sYCC",
        ImageKind::Stencil { .. } => "stencil",
        _ => panic!("unexpected image colour description"),
    }
}

fn assert_samples(samples: &Samples, expected: &[u16], name: &str) {
    assert_eq!(samples.len(), expected.len(), "{name}");
    for (i, &value) in expected.iter().enumerate() {
        assert_eq!(samples.get(i), Some(value), "{name} sample {i}");
    }
}

fn empty() -> DecodedImage {
    DecodedImage {
        width: 2,
        height: 1,
        bits_per_component: 0,
        components: 0,
        samples: Samples::Bytes(Vec::new()),
        kind: ImageKind::Color { color_space: None },
        decode: None,
        mask: MaskKind::None,
        interpolate: false,
    }
}

#[test]
fn alpha_precision_premultiplication_and_live_storage_are_preserved() {
    let component = |bits, samples| jpx::JpxComponent {
        bits,
        signed: false,
        width: 2,
        height: 1,
        samples,
    };
    let decoded = jpx::JpxImage {
        width: 2,
        height: 1,
        components: vec![component(3, vec![0, 7]), component(9, vec![0, 511])],
        color: jpx::JpxColor::Enumerated(17),
        alpha: Some(1),
    };
    let mut image = empty();
    jpx_image::adapt(decoded.clone(), 2, &mut image, &Limits::default()).unwrap();
    assert_eq!(image.bits_per_component, 3);
    assert_eq!(
        image.mask,
        MaskKind::Embedded(ImageAlpha {
            bits_per_component: 9,
            samples: Samples::Words(vec![0, 511]),
            premultiplied: true
        })
    );
    // Decoded planes, interleaved colour, opacity and both component lists
    // must fit simultaneously; the last byte still protects real storage.
    let live =
        8 + 2 + 4 + 2 * (size_of::<jpx::JpxComponent>() + size_of::<&jpx::JpxComponent>() + 1);
    let limits = Limits {
        decoded_bytes: live - 1,
        ..Limits::default()
    };
    assert_eq!(
        jpx_image::adapt(decoded.clone(), 2, &mut empty(), &limits)
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
    jpx_image::adapt(
        decoded,
        2,
        &mut empty(),
        &Limits {
            decoded_bytes: live,
            ..limits
        },
    )
    .unwrap();
}

#[test]
fn unused_opacity_layout_is_ignored() {
    let decoded = jpx::JpxImage {
        width: 2,
        height: 1,
        components: vec![
            jpx::JpxComponent {
                bits: 8,
                signed: false,
                width: 2,
                height: 1,
                samples: vec![12, 26],
            },
            jpx::JpxComponent {
                bits: 8,
                signed: false,
                width: 1,
                height: 1,
                samples: vec![0],
            },
        ],
        color: jpx::JpxColor::Enumerated(17),
        alpha: Some(1),
    };
    let mut image = empty();
    jpx_image::adapt(decoded, 0, &mut image, &Limits::default()).unwrap();
    assert_eq!(image.samples, Samples::Bytes(vec![12, 26]));
    assert_eq!(image.mask, MaskKind::None);
}
