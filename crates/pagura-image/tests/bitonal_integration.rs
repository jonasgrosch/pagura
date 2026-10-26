#![forbid(unsafe_code)]
//! Public PDF integration and bounded continuation for bitonal extensions.

use pagura_core::{ErrorKind, Limits, ObjectId, Position, Recovery};
use pagura_document::{Document, OpenOptions};
use pagura_image::{DecodeRequest, ImageSource, Samples, ccitt};
use pagura_testkit::fixture;

#[test]
fn bitonal_extensions_reach_the_pdf_image_api() {
    for (name, rows) in [
        (
            "ccitt-uncompressed-resume-vertical",
            vec![
                vec![1, 1, 1, 0, 0, 0, 0, 1, 1, 1, 1, 1],
                vec![1, 1, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1],
            ],
        ),
        (
            "jbig2-extended-template-unknown-marker-in-header",
            vec![
                vec![1; 5],
                vec![0, 1, 0, 1, 1],
                vec![0, 1, 0, 1, 1],
                vec![1; 5],
                vec![1, 0, 1, 0, 0],
            ],
        ),
    ] {
        let document = Document::open(
            fixture("image", &format!("{name}.pdf")),
            &OpenOptions::default(),
        )
        .unwrap();
        let source =
            ImageSource::from_xobject(&document, document.object(ObjectId::new(5, 0)).unwrap())
                .unwrap();
        let decoded = source
            .decode(&DecodeRequest::FULL, &document.limits(), Recovery::Strict)
            .unwrap();
        assert_eq!(decoded.image.width as usize, rows[0].len());
        assert_eq!(decoded.image.height as usize, rows.len());
        assert_eq!(decoded.image.bits_per_component, 1);
        assert_eq!(
            decoded.image.samples,
            Samples::Bytes(rows.into_iter().flatten().collect()),
            "{name}"
        );
    }
}

#[test]
fn uncompressed_continuation_obeys_exact_work_and_live_storage_limits() {
    let data = fixture("image", "fax-uncompressed-continuation-1.in");
    let params = ccitt::CcittParams {
        k: -1,
        columns: 1,
        ..ccitt::CcittParams::default()
    };
    // One entrance (10), six Table4 words (1+2+3+4+5+6) and one exit (8),
    // plus the initial six-zero optional-EOL lookahead. Those twenty pixels
    // occupy twenty one-byte rows; two three-entry transition lists and one
    // spare row coexist with them. Pending literals allocate no sample plane.
    let limits = Limits {
        operations: 45,
        pixels: 20,
        decoded_bytes: 6 * size_of::<usize>() + 21,
        stream_bytes: data.len(),
        ..Limits::default()
    };
    assert_eq!(
        ccitt::decode(&data, &params, &limits).unwrap(),
        fixture("image", "fax-uncompressed-continuation-1.out")
    );
    for limited in [
        Limits {
            operations: 44,
            ..limits
        },
        Limits {
            pixels: 19,
            ..limits
        },
        Limits {
            decoded_bytes: limits.decoded_bytes - 1,
            ..limits
        },
        Limits {
            stream_bytes: data.len() - 1,
            ..limits
        },
    ] {
        let error = ccitt::decode(&data, &params, &limited).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit);
        assert!(matches!(error.position, Position::Buffer(_)));
    }
    for end in 0..data.len() {
        assert!(ccitt::decode(&data[..end], &params, &Limits::default()).is_err());
    }
}
