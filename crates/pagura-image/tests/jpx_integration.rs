#![forbid(unsafe_code)]
//! Public image-source contracts for R02's independently authored PDF inputs.

use pagura_core::{ErrorKind, Limits, ObjectId, Recovery};
use pagura_document::{Document, OpenOptions};
use pagura_image::{DecodeRequest, ImageKind, ImageSource, MaskKind, Samples, jpx::JpxColor};

fn document(name: &str, limits: Limits) -> Document {
    Document::open(
        pagura_testkit::fixture("paint_jpx_integration", &format!("{name}.pdf")),
        &OpenOptions {
            limits,
            ..OpenOptions::default()
        },
    )
    .unwrap()
}

fn source(document: &Document) -> pagura_core::Result<ImageSource> {
    ImageSource::from_xobject(document, document.object(ObjectId::new(5, 0))?)
}

#[test]
fn ranked_candidates_remain_visible_without_decoding() {
    let document = document("precedence-signed", Limits::default());
    let source = source(&document).unwrap();
    let ImageKind::EmbeddedColor {
        color: JpxColor::Alternatives(choices),
    } = source.kind()
    else {
        panic!("missing colour candidates");
    };
    assert_eq!(
        choices
            .iter()
            .map(|c| (c.precedence, c.approximation))
            .collect::<Vec<_>>(),
        [(2, 1), (-2, 1)]
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
                let document = document(&name, Limits::default());
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
    for name in ["precedence-signed", "indexed-palette-1-1-2"] {
        let document = document(
            name,
            Limits {
                decoded_bytes: 64,
                ..Limits::default()
            },
        );
        assert!(
            matches!(source(&document).unwrap_err().kind, ErrorKind::Limit),
            "{name}"
        );
    }
}
