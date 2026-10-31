#![forbid(unsafe_code)]
//! Separator recovery cannot authorize padded or discarded sample data.
use pagura_core::{Dictionary, ErrorKind, Limits, Name, Value};
use pagura_image::validate_inline_samples;

fn gray(width: i64, height: i64, bits: i64) -> Dictionary {
    [
        (b"W".as_slice(), Value::Integer(width)),
        (b"H", Value::Integer(height)),
        (b"BPC", Value::Integer(bits)),
        (b"CS", Value::name(b"G")),
    ]
    .into_iter()
    .map(|(key, value)| (Name::new(key), value))
    .collect()
}

#[test]
fn exact_raw_rows_ignore_declared_length_and_retain_padding_bytes() {
    let mut dictionary = gray(9, 2, 1);
    dictionary.insert(Name::new(b"L"), Value::Integer(3));
    assert!(validate_inline_samples(&dictionary, &[0; 4], &Limits::default()).is_ok());
    for size in [0, 2, 3, 5] {
        assert_eq!(
            validate_inline_samples(&dictionary, &vec![0; size], &Limits::default())
                .unwrap_err()
                .kind,
            ErrorKind::Syntax
        );
    }
}

#[test]
fn ascii_filter_must_supply_all_samples_without_repair() {
    let mut dictionary = gray(4, 1, 8);
    dictionary.insert(Name::new(b"F"), Value::name(b"AHx"));
    assert!(validate_inline_samples(&dictionary, b"00ff4549>", &Limits::default()).is_ok());
    for data in [b"00ff>".as_slice(), b"00ff454978>", b"00ff4549"] {
        assert!(validate_inline_samples(&dictionary, data, &Limits::default()).is_err());
    }
}

#[test]
fn chained_filters_keep_the_caller_cap_for_intermediate_buffers() {
    let mut dictionary = gray(4, 1, 8);
    dictionary.insert(
        Name::new(b"F"),
        Value::Array(vec![Value::name(b"AHx"), Value::name(b"Fl")]),
    );
    // Independently encoded zlib "abcd" has twelve bytes before Flate.
    // The final four-byte sample size is an allocation hint, not that
    // intermediate buffer's cap.
    let data = b"789c4b4c4a4e010003d8018b>";
    assert!(validate_inline_samples(&dictionary, data, &Limits::default()).is_ok());
}

#[test]
fn actual_limits_apply_before_filter_decode_or_allocation() {
    let dictionary = gray(4, 1, 8);
    for limits in [
        Limits {
            pixels: 3,
            ..Limits::default()
        },
        Limits {
            decoded_bytes: 3,
            ..Limits::default()
        },
        Limits {
            stream_bytes: 3,
            ..Limits::default()
        },
    ] {
        assert_eq!(
            validate_inline_samples(&dictionary, &[0; 4], &limits)
                .unwrap_err()
                .kind,
            ErrorKind::Limit
        );
    }
}

#[test]
fn codec_proof_does_not_duplicate_per_image_codec_work() {
    for filter in [b"DCT".as_slice(), b"CCF"] {
        let mut dictionary = gray(4, 1, 8);
        dictionary.insert(Name::new(b"F"), Value::name(filter));
        assert_eq!(
            validate_inline_samples(&dictionary, &[0; 4], &Limits::default())
                .unwrap_err()
                .kind,
            ErrorKind::Syntax
        );
    }
}
