#![forbid(unsafe_code)]
//! Public image-source contracts for R02's independently authored PDF inputs.

use pagura_core::{ErrorKind, Limits, ObjectId, Recovery};
use pagura_document::{Document, OpenOptions};
use pagura_image::{DecodeRequest, ImageKind, ImageSource, MaskKind, Samples, jpx::JpxColor};

fn document(name: &str, limits: Limits) -> pagura_core::Result<Document> {
    Document::open(
        pagura_testkit::fixture("paint_jpx_integration", &format!("{name}.pdf")),
        &OpenOptions {
            limits,
            ..OpenOptions::default()
        },
    )
}

fn source(document: &Document) -> pagura_core::Result<ImageSource> {
    ImageSource::from_xobject(document, document.object(ObjectId::new(5, 0))?)
}

#[test]
fn main_header_markers_and_jpxb_requirements_preserve_public_samples() {
    let expected: Vec<_> = [0, 64, 128, 255, 255, 128, 64, 0].repeat(8);
    for name in [
        "jpx-plm",
        "jpx-crg-zero",
        "jpx-plm-crg-zero",
        "jpxb-only",
        "jpxb-jp2-control",
    ] {
        let document = Document::open(
            pagura_testkit::fixture("image", &format!("{name}.pdf")),
            &OpenOptions::default(),
        )
        .unwrap();
        let image = source(&document)
            .unwrap()
            .decode(&DecodeRequest::FULL, &document.limits(), Recovery::Strict)
            .unwrap()
            .image;
        assert_eq!((image.width, image.height), (8, 8));
        assert_eq!(image.samples, Samples::Bytes(expected.clone()), "{name}");
    }
}

#[test]
fn cielab_metadata_preserves_post_palette_precision_and_raw_samples() {
    for (name, bits, first) in [
        ("lab-default-8", [8; 3], [127, 128, 96]),
        ("lab-explicit-default-8", [8; 3], [127, 128, 96]),
        ("lab-default-16", [16; 3], [32767, 32768, 24576]),
        ("lab-explicit-custom-8", [8; 3], [64, 96, 160]),
        ("lab-explicit-custom-16", [16; 3], [16384, 24576, 40960]),
        (
            "lab-default-palette-8-12-16",
            [8, 12, 16],
            [127, 2048, 24576],
        ),
        ("lab-default-1bit", [1; 3], [0; 3]),
        ("lab-default-2bit", [2; 3], [0; 3]),
        ("lab-palette-1-2-16", [1, 2, 16], [0; 3]),
    ] {
        let document = Document::open(
            pagura_testkit::fixture("jpx_lab_probe", &format!("{name}.pdf")),
            &OpenOptions::default(),
        )
        .unwrap();
        let source = source(&document).unwrap();
        assert_eq!(source.components(), 3);
        for (index, precision) in bits.into_iter().enumerate() {
            assert_eq!(source.component_bits(index), Some(precision), "{name}");
        }
        let ImageKind::EmbeddedColor { color, sample_bias } = source.kind() else {
            panic!("missing Lab metadata")
        };
        assert!(matches!(color, JpxColor::Enumerated(14) | JpxColor::Lab(_)));
        assert_eq!(sample_bias.as_slice(), []);
        for layout in [
            pagura_image::Layout::Interleaved,
            pagura_image::Layout::Native,
        ] {
            let decoded = source
                .decode(
                    &DecodeRequest::new(pagura_image::Reduce::Full, None, layout),
                    &document.limits(),
                    Recovery::Strict,
                )
                .unwrap();
            assert_eq!(&decoded.image.kind, source.kind());
            assert!(decoded.image.embedded_color_fallback().is_none());
            for (index, expected) in first.into_iter().enumerate() {
                assert_eq!(decoded.image.samples.get(index), Some(expected), "{name}");
            }
        }
    }
}

#[test]
fn cielab_parameter_lengths_and_metadata_budgets_are_explicit() {
    for length in [1, 4, 24, 27, 29, 32] {
        let data =
            pagura_testkit::fixture("jpx_lab_probe", &format!("lab-malformed-ep-{length}.jpx"));
        let error = pagura_image::jpx::decode(&data, &Limits::default()).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax);
        assert!(matches!(error.position, pagura_core::Position::Buffer(_)));
    }
    for name in ["lab-explicit-custom-8", "lab-default-palette-8-12-16"] {
        let document = Document::open(
            pagura_testkit::fixture("jpx_lab_probe", &format!("{name}.pdf")),
            &OpenOptions {
                limits: Limits {
                    decoded_bytes: 64,
                    ..Limits::default()
                },
                ..OpenOptions::default()
            },
        )
        .unwrap();
        assert_eq!(source(&document).unwrap_err().kind, ErrorKind::Limit);
    }
}

#[test]
fn ranked_candidates_remain_visible_without_decoding() {
    let document = document("any-icc-precedence-signed", Limits::default()).unwrap();
    let source = source(&document).unwrap();
    let ImageKind::EmbeddedColor {
        color: JpxColor::Alternatives(choices),
        ..
    } = source.kind()
    else {
        panic!("missing colour candidates");
    };
    assert_eq!(
        choices
            .iter()
            .map(|c| (c.precedence, c.approximation))
            .collect::<Vec<_>>(),
        [(2, 1), (-2, 4), (-128, 1)]
    );
    assert!(
        choices
            .iter()
            .all(|choice| matches!(&choice.color, JpxColor::Icc(data) if data.len() > 128))
    );
    assert!(
        source.retained_bytes()
            >= choices
                .iter()
                .map(|c| c.color.retained_bytes())
                .sum::<usize>()
    );
    let decoded = source
        .decode(&DecodeRequest::FULL, &document.limits(), Recovery::Strict)
        .unwrap();
    assert_eq!(&decoded.image.kind, source.kind());
}

#[test]
fn pdf_index_and_opacity_have_separate_sample_precision_and_order() {
    for mapped in [false, true] {
        for first in [false, true] {
            for mode in 0..=2 {
                let name = format!(
                    "indexed-palette-{}-{}-{mode}",
                    u8::from(mapped),
                    u8::from(first)
                );
                let document = document(&name, Limits::default()).unwrap();
                let source = source(&document).unwrap();
                assert_eq!(
                    (source.components(), source.component_bits(0)),
                    (1, Some(8))
                );
                let decoded = source
                    .decode(&DecodeRequest::FULL, &document.limits(), Recovery::Strict)
                    .unwrap();
                assert_eq!(
                    decoded.image.samples,
                    Samples::Bytes(vec![0, 1, 2]),
                    "{name}"
                );
                match decoded.image.mask {
                    MaskKind::None if mode == 0 => {}
                    MaskKind::Embedded(alpha) if mode != 0 => {
                        assert_eq!(alpha.premultiplied, mode == 2, "{name}");
                        assert_eq!(
                            alpha.bits_per_component,
                            if mapped { 16 } else { 8 },
                            "{name}"
                        );
                        assert_eq!(
                            alpha.samples,
                            if mapped {
                                Samples::Words(vec![0, 32768, 65535])
                            } else {
                                Samples::Bytes(vec![0, 128, 255])
                            },
                            "{name}"
                        );
                    }
                    other => panic!("{name}: {other:?}"),
                }
            }
        }
    }
}

#[test]
fn candidate_metadata_and_palette_adaptation_obey_output_limits() {
    for name in ["any-icc-precedence-signed", "indexed-palette-1-1-2"] {
        let document = document(
            name,
            Limits {
                decoded_bytes: 64,
                ..Limits::default()
            },
        )
        .unwrap();
        assert!(
            matches!(source(&document).unwrap_err().kind, ErrorKind::Limit),
            "{name}"
        );
    }
}

#[test]
fn signed_profile_samples_preserve_precision_and_storage_bias() {
    for bits in [8, 16] {
        for (name, count) in [("gray", 1), ("rgb", 3)] {
            let document =
                document(&format!("signed-trc-{name}-{bits}"), Limits::default()).unwrap();
            let source = source(&document).unwrap();
            let ImageKind::EmbeddedColor { sample_bias, .. } = source.kind() else {
                panic!("missing embedded sample metadata");
            };
            let bias = 1_u16 << (bits - 1);
            assert_eq!(sample_bias, &vec![bias; count]);
            for component in 0..count {
                assert_eq!(source.component_bits(component), Some(bits));
            }
            let decoded = source
                .decode(&DecodeRequest::FULL, &document.limits(), Recovery::Strict)
                .unwrap();
            assert_eq!(&decoded.image.kind, source.kind());
            assert!(decoded.image.embedded_color_fallback().is_none());
            let maximum = u16::MAX >> (16 - bits);
            for (pixel, expected) in [0, bias / 2, bias - 1, bias, bias + bias / 2, maximum]
                .into_iter()
                .enumerate()
            {
                assert_eq!(decoded.image.samples.get(pixel * count), Some(expected));
            }
        }
    }
}

#[test]
fn signed_palette_components_keep_independent_bias_and_unsigned_opacity() {
    let document = document("signed-trc-mixed-alpha-2", Limits::default()).unwrap();
    let source = source(&document).unwrap();
    let ImageKind::EmbeddedColor { sample_bias, .. } = source.kind() else {
        panic!("missing palette colour metadata");
    };
    assert_eq!(sample_bias, &[128, 0, 32768]);
    assert_eq!(
        (0..3).map(|i| source.component_bits(i)).collect::<Vec<_>>(),
        [Some(8), Some(10), Some(16)]
    );
    let decoded = source
        .decode(&DecodeRequest::FULL, &document.limits(), Recovery::Strict)
        .unwrap();
    assert_eq!(
        decoded.image.samples,
        Samples::Mixed {
            values: vec![64, 0, 31768, 160, 128, 40960, 192, 256, 49152],
            bits: vec![8, 10, 16],
        }
    );
    let MaskKind::Embedded(alpha) = decoded.image.mask else {
        panic!("missing unsigned opacity");
    };
    assert_eq!(alpha.bits_per_component, 8);
    assert_eq!(alpha.samples, Samples::Bytes(vec![0, 128, 255]));
    assert!(alpha.premultiplied);
}
