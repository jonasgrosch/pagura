//! PDF image/filter integration oracles, calculated by the fixture generator.
use crate::{DecodedImage, ImageAlpha, ImageKind, MaskKind, Samples, jpx, jpx_image};
use pagura_core::{ErrorKind, Limits, ObjectId, Position, Recovery, Value};
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
        &OpenOptions {
            recovery: if params["outcome"] == "repair" {
                Recovery::Repair
            } else {
                Recovery::Strict
            },
            ..OpenOptions::default()
        },
    )
    .unwrap();
    let result = crate::decode_xobject(&document, document.object(ObjectId::new(3, 0)).unwrap());
    if params["outcome"] == "repair" {
        assert_repairs(name, params, &document);
    } else if params["outcome"] != "valid" {
        assert_rejected(name, params, &result.unwrap_err());
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
    if let Some(inverted) = params.get("inverted") {
        assert_eq!(
            image.kind,
            ImageKind::Stencil {
                inverted: inverted == "1"
            },
            "{name}"
        );
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
        assert_eq!(
            alpha.premultiplied,
            params.get("premultiplied").is_some_and(|v| v == "1")
        );
        assert_samples(
            &alpha.samples,
            &values(&fixture("image", &format!("{name}.alpha"))),
            name,
        );
    } else {
        assert_eq!(image.mask, MaskKind::None, "{name}");
    }
}

fn assert_rejected(name: &str, params: &BTreeMap<String, String>, error: &pagura_core::Error) {
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
    if params.get("reject_repair").is_some_and(|v| v == "1") {
        let repaired = Document::open(
            fixture("image", &format!("{name}.pdf")),
            &OpenOptions::default(),
        )
        .unwrap();
        let refused =
            crate::decode_xobject(&repaired, repaired.object(ObjectId::new(3, 0)).unwrap())
                .unwrap_err();
        assert_eq!(refused.kind, error.kind, "{name}");
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
    jpx_image::adapt(placed(decoded.clone()), 2, &mut image, &Limits::default()).unwrap();
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
        jpx_image::adapt(placed(decoded.clone()), 2, &mut empty(), &limits)
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
    jpx_image::adapt(
        placed(decoded),
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
    jpx_image::adapt(placed(decoded), 0, &mut image, &Limits::default()).unwrap();
    assert_eq!(image.samples, Samples::Bytes(vec![12, 26]));
    assert_eq!(image.mask, MaskKind::None);
}

fn placed(image: jpx::JpxImage) -> jpx::Decoded {
    jpx::Decoded {
        component_geometry: vec![
            jpx::ComponentGeometry {
                step: [1, 1],
                origin: [0, 0]
            };
            image.components.len()
        ],
        reference_origin: [0, 0],
        opacity: None,
        image,
    }
}

fn assert_repairs(name: &str, params: &BTreeMap<String, String>, document: &Document) {
    let strict = Document::open(
        fixture("image", &format!("{name}.pdf")),
        &OpenOptions {
            recovery: Recovery::Strict,
            ..OpenOptions::default()
        },
    )
    .unwrap();
    let error =
        crate::decode_xobject(&strict, strict.object(ObjectId::new(3, 0)).unwrap()).unwrap_err();
    assert_eq!(error.repair.unwrap().as_str(), params["repair"], "{name}");
    assert_eq!(error.object, Some(ObjectId::new(3, 0)), "{name}");
    assert!(matches!(error.position, Position::Stream { .. }), "{name}");
    assert!(strict.diagnostics().is_empty(), "{name}");
    let reports = document.diagnostics();
    let expected_tags = params.get("report_tags").map_or_else(
        || vec![params["repair"].as_str(); params.get("reports").map_or(1, |n| n.parse().unwrap())],
        |tags| tags.split(',').collect(),
    );
    assert_eq!(
        reports
            .iter()
            .map(|report| report.repair.as_str())
            .collect::<Vec<_>>(),
        expected_tags,
        "{name}"
    );
    for report in &reports {
        assert_eq!(report.object, Some(ObjectId::new(3, 0)), "{name}");
        assert!(matches!(report.position, Position::Stream { .. }), "{name}");
    }
    // A second use of the same XObject must not duplicate reports.
    crate::decode_xobject(document, document.object(ObjectId::new(3, 0)).unwrap()).unwrap();
    assert_eq!(document.diagnostics(), reports, "{name}");
}

#[test]
fn jpeg_reports_are_bounded_per_decode_and_do_not_authorize_strict_repairs() {
    let document = Document::open(
        fixture("image", "jpeg-padding-progressive.pdf"),
        &OpenOptions::default(),
    )
    .unwrap();
    let stream = document
        .image_stream(document.object(ObjectId::new(3, 0)).unwrap())
        .unwrap();
    let mut diagnostics = pagura_core::Diagnostics::new(Recovery::Repair);
    for _ in 0..2 {
        let image = crate::dct::decode_with_diagnostics(
            &stream.data,
            None,
            &Limits::default(),
            &mut diagnostics,
        )
        .unwrap();
        assert_eq!(image.samples, [128]);
    }
    // Multiple repaired boundaries in each progressive JPEG produce one
    // record; a second decode still obtains its own permission and evidence.
    assert_eq!(diagnostics.as_slice().len(), 2);
    let mut strict = pagura_core::Diagnostics::new(Recovery::Strict);
    strict.extend(diagnostics.take());
    let error =
        crate::dct::decode_with_diagnostics(&stream.data, None, &Limits::default(), &mut strict)
            .unwrap_err();
    assert_eq!(error.repair, Some(crate::dct::tags::ENTROPY_PADDING));
}

#[test]
fn retained_jpx_metadata_capacity_counts_against_the_adapter_budget() {
    for geometry in [false, true] {
        let mut decoded = placed(jpx::JpxImage {
            width: 2,
            height: 1,
            components: vec![jpx::JpxComponent {
                bits: 8,
                signed: false,
                width: 2,
                height: 1,
                samples: vec![12, 26],
            }],
            color: jpx::JpxColor::Enumerated(17),
            alpha: None,
        });
        if geometry {
            decoded.component_geometry.reserve_exact(127);
        } else {
            decoded.image.components.reserve_exact(127);
        }
        let error = jpx_image::adapt(
            decoded,
            0,
            &mut empty(),
            &Limits {
                decoded_bytes: 1024,
                ..Limits::default()
            },
        )
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit);
    }
}

#[test]
fn zero_byte_fill_preserves_the_high_byte_of_an_incomplete_word() {
    let mut diagnostics = pagura_core::Diagnostics::new(Recovery::Repair);
    let samples = crate::parameters::unpack_with_diagnostics(
        &[0x12, 0x34, 0xab],
        2,
        1,
        1,
        16,
        &Limits::default(),
        &mut diagnostics,
    )
    .unwrap();
    assert_eq!(samples, Samples::Words(vec![0x1234, 0xab00]));
    assert_eq!(diagnostics.as_slice()[0].repair, crate::tags::DATA_SHORT);
}
