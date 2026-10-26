#![forbid(unsafe_code)]
//! Public lossless JPEG compatibility and bounded PDF image decoding.

use pagura_core::{ErrorKind, Limits, ObjectId, Position, Recovery};
use pagura_document::{Document, OpenOptions};
use pagura_image::{DecodeRequest, ImageSource, Samples, dct};
use pagura_testkit::fixture;

#[test]
fn lossless_jpeg_samples_reach_the_pdf_image_api() {
    for (image, vector) in [
        ("gray", "predictor-1-8"),
        ("16bit", "predictor-1-16"),
        ("half-predictor", "predictor-6-8"),
        ("rgb-subsampled", "subsampled"),
        ("rgb-fractional", "fractional"),
        ("ycbcr", "ycbcr"),
    ] {
        let document = Document::open(
            fixture("image", &format!("jpeg-lossless-huffman-{image}.pdf")),
            &OpenOptions::default(),
        )
        .unwrap();
        let source =
            ImageSource::from_xobject(&document, document.object(ObjectId::new(5, 0)).unwrap())
                .unwrap();
        let decoded = source
            .decode(&DecodeRequest::FULL, &document.limits(), Recovery::Strict)
            .unwrap();
        assert_eq!((decoded.image.width, decoded.image.height), (5, 4));
        assert_eq!(decoded.image.bits_per_component, 8);
        assert_eq!(
            decoded.image.samples,
            Samples::Bytes(fixture(
                "image",
                &format!("jpeg-lossless-vector-{vector}.out")
            )),
            "{image}"
        );
    }
}

#[test]
fn lossless_sample_storage_and_work_have_exact_boundaries() {
    let data = fixture("image", "jpeg-lossless-vector-predictor-1-8.in");
    // Twenty u16 native samples coexist with twenty interleaved u16
    // samples and twenty output bytes. Each sample is one entropy data unit;
    // the shared scan terminator additionally inspects FF and D9 (two units).
    let limits = Limits {
        decoded_bytes: 100,
        operations: 22,
        pixels: 20,
        ..Limits::default()
    };
    assert_eq!(
        dct::decode(&data, Some(0), &limits).unwrap().samples,
        fixture("image", "jpeg-lossless-vector-predictor-1-8.out")
    );
    for limited in [
        Limits {
            decoded_bytes: 99,
            ..limits
        },
        Limits {
            operations: 21,
            ..limits
        },
        Limits {
            pixels: 19,
            ..limits
        },
    ] {
        let error = dct::decode(&data, Some(0), &limited).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit);
        assert!(matches!(error.position, Position::Buffer(_)));
    }
}

#[test]
fn lossless_truncations_and_mutations_stay_bounded() {
    let limits = Limits {
        pixels: 4096,
        decoded_bytes: 1 << 20,
        operations: 100_000,
        ..Limits::default()
    };
    for name in ["predictor-1-16", "subsampled", "rgb-planar"] {
        let data = fixture("image", &format!("jpeg-lossless-vector-{name}.in"));
        for end in 0..data.len() {
            let error = dct::decode(&data[..end], Some(0), &limits).unwrap_err();
            assert!(
                matches!(error.position, Position::Buffer(_)),
                "{name}: {end}"
            );
        }
        for index in 0..data.len() {
            let mut damaged = data.clone();
            damaged[index] ^= 0x80;
            let _ = dct::decode(&damaged, Some(0), &limits);
        }
    }
}
