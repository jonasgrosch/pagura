//! Entropy-process resource and malformed-input regressions.
use pagura_core::{Diagnostics, ErrorKind, Limits, Position, Recovery};
use pagura_testkit::fixture;

#[test]
fn arithmetic_and_twelve_bit_mutations_remain_bounded() {
    let limits = Limits {
        pixels: 4096,
        decoded_bytes: 1 << 20,
        decode_bytes: 1 << 20,
        decode_work: 100_000,
        ..Limits::default()
    };
    for name in [
        "jpeg-arithmetic-sequential",
        "jpeg-arithmetic-progressive",
        "jpeg-arithmetic-progressive-four-tables",
        "jpeg-12bit-progressive",
    ] {
        let encoded = fixture("image", &format!("{name}.in"));
        for end in 0..encoded.len() {
            let error = super::decode(&encoded[..end], Some(0), &limits).unwrap_err();
            assert!(
                matches!(error.position, Position::Buffer(_)),
                "{name} prefix {end}"
            );
        }
        for index in 0..encoded.len() {
            let mut damaged = encoded.clone();
            damaged[index] ^= 0x80;
            let _ = super::decode(&damaged, Some(0), &limits);
        }
    }
}

#[test]
fn arithmetic_work_coefficients_and_samples_are_budgeted() {
    let data = fixture("image", "jpeg-arithmetic-sequential.in");
    for limits in [
        Limits {
            decode_work: 100,
            ..Limits::default()
        },
        Limits {
            decode_bytes: 100,
            ..Limits::default()
        },
        Limits {
            decoded_bytes: 100,
            ..Limits::default()
        },
        Limits {
            pixels: 100,
            ..Limits::default()
        },
    ] {
        let error = super::decode(&data, None, &limits).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit);
        assert!(matches!(error.position, Position::Buffer(_)));
    }
}

#[test]
fn arithmetic_and_huffman_twins_share_the_block_work_budget() {
    // Both twins code 128 blocks in one scan: 64 units each. Reading the
    // marker after Huffman data costs one more unit.
    let blocks = 128 * 64;
    let huffman = fixture("image", "jpeg-budget-huffman.in");
    let arithmetic = fixture("image", "jpeg-budget-arithmetic.in");
    let limits = Limits {
        decode_work: blocks + 1,
        ..Limits::default()
    };
    let expected = super::decode(&huffman, Some(0), &limits).unwrap();
    assert_eq!(
        super::decode(&arithmetic, Some(0), &limits).unwrap(),
        expected
    );
    for data in [&huffman, &arithmetic] {
        let error = super::decode(
            data,
            Some(0),
            &Limits {
                decode_work: blocks - 1,
                ..limits
            },
        )
        .unwrap_err();
        assert_eq!(
            (error.kind, error.message),
            (ErrorKind::Limit, "JPEG work budget exceeded")
        );
    }
}

#[test]
fn twelve_bit_fixtures_round_once_against_the_native_precision_model() {
    let mut count = 0;
    for entry in std::fs::read_dir(pagura_testkit::fixture_path("image", "")).unwrap() {
        let path = entry.unwrap().path();
        if path
            .extension()
            .is_none_or(|extension| extension != "exact")
        {
            continue;
        }
        let name = path.file_stem().unwrap().to_str().unwrap();
        let params = String::from_utf8(fixture("image", &format!("{name}.params"))).unwrap();
        let transform = params
            .lines()
            .find_map(|line| line.strip_prefix("transform="))
            .map(|value| value.parse().unwrap());
        let decoded = super::decode(
            &fixture("image", &format!("{name}.in")),
            transform,
            &Limits::default(),
        )
        .unwrap();
        let exact = std::fs::read(&path).unwrap();
        assert_eq!(exact.len(), decoded.samples.len() * 8, "{name}");
        let mut maximum = 0.0f64;
        for (index, (&actual, bytes)) in decoded
            .samples
            .iter()
            .zip(exact.as_chunks::<8>().0)
            .enumerate()
        {
            let expected = f64::from_le_bytes(*bytes);
            let error = (f64::from(actual) - expected).abs();
            assert!(
                error <= 0.5,
                "{name} sample {index}: {actual} vs {expected} ({error})"
            );
            maximum = maximum.max(error);
        }
        println!("{name}: maximum error {maximum:.9} levels");
        count += 1;
    }
    assert!(count >= 25, "missing native-precision fixtures");
}

#[test]
fn missing_quantizer_reports_the_entropy_data_position() {
    for name in ["jpeg-arithmetic-sequential", "jpeg-12bit-sequential"] {
        let mut encoded = fixture("image", &format!("{name}.in"));
        let table = encoded
            .windows(2)
            .position(|pair| pair == [255, 219])
            .unwrap();
        let length = usize::from(u16::from_be_bytes([encoded[table + 2], encoded[table + 3]]));
        encoded.drain(table..table + length + 2);
        let scan = encoded
            .windows(2)
            .position(|pair| pair == [255, 218])
            .unwrap();
        let length = usize::from(u16::from_be_bytes([encoded[scan + 2], encoded[scan + 3]]));
        let error = super::decode(&encoded, Some(0), &Limits::default()).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax);
        assert_eq!(error.position, Position::Buffer(scan + 2 + length));
    }
}

#[test]
fn twelve_bit_native_buffers_are_admitted_before_allocation() {
    let data = fixture("image", "jpeg-12bit-sequential.in");
    // Six 8x8 blocks: i32 coefficients, u16 planes and expanded u16 samples
    // are working memory; the bytes are the output.
    let samples = 24 * 16;
    let limits = Limits {
        decode_bytes: samples * (4 + 2 + 2),
        decoded_bytes: samples,
        ..Limits::default()
    };
    assert!(super::decode(&data, Some(0), &limits).is_ok());
    for capped in [
        Limits {
            decode_bytes: limits.decode_bytes - 1,
            ..limits
        },
        Limits {
            decoded_bytes: limits.decoded_bytes - 1,
            ..limits
        },
    ] {
        let error = super::decode(&data, Some(0), &capped).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit);
    }
}

#[test]
fn headers_read_without_decoding_and_restart_intervals_have_two_bytes() {
    let data = fixture("image", "jpeg-baseline-gray.in");
    let decoded = super::decode(&data, None, &Limits::default()).unwrap();
    let header = super::header(&data).unwrap();
    assert_eq!(
        (header.width, header.height, header.components),
        (decoded.width, decoded.height, decoded.components)
    );
    // A DRI segment with a three-byte payload, inserted after SOI.
    let mut damaged = data[..2].to_vec();
    damaged.extend_from_slice(&[0xff, 0xdd, 0x00, 0x05, 0x00, 0x01, 0x00]);
    damaged.extend_from_slice(&data[2..]);
    let error = super::decode(&damaged, None, &Limits::default()).unwrap_err();
    assert_eq!(
        (error.message, error.position),
        ("invalid JPEG restart interval length", Position::Buffer(2))
    );
    assert_eq!(
        super::header(&data[..2]).unwrap_err().kind,
        ErrorKind::Syntax
    );
}

#[test]
fn a_restart_marker_directly_before_eoi_is_a_reported_repair() {
    // After the final MCU no restart interval remains (T.81 B.2.1, B.2.4.4).
    for name in ["jpeg-baseline-gray", "jpeg-restart", "jpeg-progressive"] {
        let data = fixture("image", &format!("{name}.in"));
        let expected = super::decode(&data, None, &Limits::default()).unwrap();
        let end = data.len() - 2;
        let with_tail = |tail: &[u8]| [&data[..end], tail].concat();
        let damaged = with_tail(&[0xff, 0xd2, 0xff, 0xd9]);
        let mut diagnostics = Diagnostics::new(Recovery::Repair);
        let decoded =
            super::decode_with_diagnostics(&damaged, None, &Limits::default(), &mut diagnostics)
                .unwrap();
        assert_eq!(decoded, expected, "{name}");
        let [report] = diagnostics.as_slice() else {
            panic!("{name}: {diagnostics:?}");
        };
        assert_eq!(
            (report.repair(), report.position),
            (
                Some(crate::repairs::JPEG_TRAILING_RESTART),
                Position::Buffer(end)
            ),
            "{name}"
        );
        let error = super::decode(&damaged, None, &Limits::default()).unwrap_err();
        assert_eq!(
            error.repair,
            Some(crate::repairs::JPEG_TRAILING_RESTART),
            "{name}"
        );
        // Anything between the restart marker and EOI keeps it an error.
        let separated = with_tail(&[0xff, 0xd2, 0x00, 0xff, 0xd9]);
        let mut diagnostics = Diagnostics::new(Recovery::Repair);
        assert!(
            super::decode_with_diagnostics(&separated, None, &Limits::default(), &mut diagnostics)
                .is_err(),
            "{name}"
        );
    }
}
