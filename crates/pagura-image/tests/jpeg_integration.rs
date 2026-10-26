#![forbid(unsafe_code)]
//! Public JPEG DNL/lossless compatibility and bounded PDF image decoding.

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
    // First-scan DNL discovery inspects SOS/length (4 bytes) and its entropy
    // through EOI (34 bytes). It allocates no plane or entropy copy.
    let limits = Limits {
        decoded_bytes: 100,
        operations: 60,
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
            operations: 59,
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
fn dnl_resolves_height_before_allocation_and_uses_exact_work_limits() {
    // DHT and SOS marker/length fields: 4+4 bytes; entropy and DNL marker:
    // 1+2; Ld and NL: 4. Decoding one DCT block costs 64 plus the final
    // two-byte marker inspection: 15+64+2=81 operations. Admission includes
    // 64 native and 64 output bytes, plus the DCT conversion-row allowance:
    // six i16 rows and one (index, two bytes) tie row. This conservative
    // allowance is retained even though grayscale does not allocate the rows.
    let bytes = 128 + 8 * (6 * size_of::<i16>() + size_of::<(usize, u8, u8)>());
    let limits = Limits {
        decoded_bytes: bytes,
        operations: 81,
        pixels: 64,
        ..Limits::default()
    };
    for name in ["deferred", "same-height", "redefined"] {
        let data = fixture("image", &format!("jpeg-dnl-vector-{name}.in"));
        let decoded = dct::decode(&data, Some(0), &limits).unwrap();
        assert_eq!((decoded.width, decoded.height), (8, 8));
        assert_eq!(decoded.samples, [128; 64]);
        for limited in [
            Limits {
                decoded_bytes: bytes - 1,
                ..limits
            },
            Limits {
                operations: 80,
                ..limits
            },
            Limits {
                pixels: 63,
                ..limits
            },
        ] {
            assert_eq!(
                dct::decode(&data, Some(0), &limited).unwrap_err().kind,
                ErrorKind::Limit
            );
        }
    }
}

#[test]
fn dnl_samples_reach_both_pdf_image_layouts() {
    for name in ["deferred", "same-height", "redefined", "progressive"] {
        let document = Document::open(
            fixture("image", &format!("jpeg-dnl-{name}.pdf")),
            &OpenOptions::default(),
        )
        .unwrap();
        let source =
            ImageSource::from_xobject(&document, document.object(ObjectId::new(5, 0)).unwrap())
                .unwrap();
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
            assert_eq!((decoded.image.width, decoded.image.height), (8, 8));
            assert_eq!(decoded.image.samples, Samples::Bytes(vec![128; 64]));
        }
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
