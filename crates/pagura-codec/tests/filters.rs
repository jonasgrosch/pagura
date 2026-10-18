//! Filter names, parameters and chains through the public API (clause 7.4).

use pagura_codec::{Filter, FilterChain, PredictorKind};
use pagura_core::{Dictionary, Name, Value};

#[test]
fn chains_pair_filters_with_parameters_and_borrow_plain_data() {
    let params: Dictionary = [
        (Name::new(b"Predictor"), Value::Integer(12)),
        (Name::new(b"Columns"), Value::Integer(5)),
    ]
    .into_iter()
    .collect();
    let chain = FilterChain::from_entries(
        Some(&Value::Array(vec![Value::name(b"A85"), Value::name(b"Fl")])),
        Some(&Value::Array(vec![Value::Null, Value::Dictionary(params)])),
    )
    .unwrap();
    assert_eq!(chain.filters()[0], Filter::Ascii85);
    let Filter::Flate { predictor } = chain.filters()[1] else {
        panic!("expected Flate");
    };
    assert_eq!(
        (predictor.kind(), predictor.columns()),
        (PredictorKind::Png, 5)
    );
    let plain = FilterChain::default();
    let mut diagnostics = pagura_core::Diagnostics::default();
    let decoded = plain.decode(b"abc", 3, &mut diagnostics).unwrap();
    assert!(matches!(decoded, std::borrow::Cow::Borrowed(b"abc")));
}

#[test]
fn validates_parameters_and_preserves_image_and_security_boundaries() {
    use pagura_codec::{Predictor, features};
    use pagura_core::{Diagnostics, ErrorKind};
    for (key, value) in [
        (b"Colors".as_slice(), 0),
        (b"Columns", -1),
        (b"BitsPerComponent", 3),
        (b"Predictor", 9),
        (b"EarlyChange", 2),
    ] {
        let params = [(Name::new(key), Value::Integer(value))]
            .into_iter()
            .collect();
        assert_eq!(
            Filter::from_pdf(b"LZWDecode", Some(&params))
                .unwrap_err()
                .kind,
            ErrorKind::Syntax
        );
    }
    for (key, value) in [
        (b"Colors".as_slice(), 256),
        (b"Columns", i64::from(u32::MAX) + 1),
    ] {
        let params = [(Name::new(key), Value::Integer(value))]
            .into_iter()
            .collect();
        assert_eq!(
            Predictor::from_params(Some(&params)).unwrap_err().kind,
            ErrorKind::Unsupported(features::PREDICTOR_DIMENSIONS)
        );
    }
    for value in 10..=15 {
        let params = [(Name::new(b"Predictor"), Value::Integer(value))]
            .into_iter()
            .collect();
        assert_eq!(
            Predictor::from_params(Some(&params)).unwrap().kind(),
            PredictorKind::Png
        );
    }
    for (name, tag) in [
        (b"DCTDecode".as_slice(), features::DCT),
        (b"JPXDecode", features::JPX),
        (b"JBIG2Decode", features::JBIG2),
        (b"CCITTFaxDecode", features::CCITT_FAX),
    ] {
        let filter = Filter::from_pdf(name, None).unwrap();
        assert_eq!(
            filter
                .decode(b"", 0, &mut Diagnostics::default())
                .unwrap_err()
                .kind,
            ErrorKind::Unsupported(tag)
        );
    }
    let crypt = Filter::from_pdf(b"Crypt", None).unwrap();
    assert_eq!(
        crypt
            .decode(b"", 0, &mut Diagnostics::default())
            .unwrap_err()
            .kind,
        ErrorKind::InvalidArgument
    );
    let mut chain = FilterChain::from_entries(
        Some(&Value::Array(vec![
            Value::name(b"Crypt"),
            Value::name(b"AHx"),
        ])),
        None,
    )
    .unwrap();
    assert_eq!(chain.take_crypt(), Some(None));
    assert_eq!(chain.take_crypt(), None);
    assert_eq!(
        chain
            .decode(b"6162>", 2, &mut Diagnostics::default())
            .unwrap()
            .as_ref(),
        b"ab"
    );
}

#[test]
fn chained_filters_use_ordered_steps_with_a_cap_at_each_step() {
    let chain = FilterChain::from_entries(
        Some(&Value::Array(vec![Value::name(b"AHx"), Value::name(b"RL")])),
        None,
    )
    .unwrap();
    let mut diagnostics = pagura_core::Diagnostics::default();
    assert_eq!(
        chain
            .decode(b"ff4180>", 3, &mut diagnostics)
            .unwrap()
            .as_ref(),
        b"AA"
    );
    assert!(diagnostics.is_empty());
    assert_eq!(
        chain
            .decode(b"ff4180>", 2, &mut diagnostics)
            .unwrap_err()
            .kind,
        pagura_core::ErrorKind::Limit
    );
}

#[test]
fn malformed_small_inputs_never_panic_or_exceed_cap() {
    let filters = [
        Filter::AsciiHex,
        Filter::Ascii85,
        Filter::RunLength,
        Filter::Flate {
            predictor: pagura_codec::Predictor::default(),
        },
        Filter::Lzw {
            predictor: pagura_codec::Predictor::default(),
            early_change: pagura_codec::EarlyChange::Postponed,
        },
        Filter::Lzw {
            predictor: pagura_codec::Predictor::default(),
            early_change: pagura_codec::EarlyChange::Early,
        },
    ];
    // Exhaustive two-byte input exercises invalid characters, headers, codes,
    // missing markers and truncated bit reads. No expected-output self oracle.
    for number in 0..=u16::MAX {
        let input = number.to_le_bytes();
        for filter in &filters {
            let result = filter.decode(&input, 8, &mut pagura_core::Diagnostics::default());
            if let Ok(bytes) = result {
                assert!(bytes.len() <= 8);
            }
        }
    }
}
