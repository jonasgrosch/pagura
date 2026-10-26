//! Entropy-process resource and malformed-input regressions.
use pagura_core::{ErrorKind, Limits, Position};
use pagura_testkit::fixture;

#[test]
fn first_scan_framing_shares_inline_stuffing_restart_and_work_rules() {
    let data = [1, 255, 0, 0xdc, 255, 255, 0xd0];
    let mut work = 0;
    let marker = super::scan::entropy_marker(&data, 0, &mut work, 7).unwrap();
    assert_eq!((marker.start, marker.end, marker.code), (4, 7, 0xd0));
    assert_eq!(work, 7);
    assert_eq!(
        super::scan::entropy_marker(&data, 0, &mut 0, 6)
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
    assert_eq!(
        super::scan::entropy_marker(&[255, 255, 0], 0, &mut 0, 10)
            .unwrap_err()
            .kind,
        ErrorKind::Syntax
    );
    for name in ["deferred", "progressive"] {
        let data = fixture("image", &format!("jpeg-dnl-vector-{name}.in"));
        let mut content = data.clone();
        content.extend_from_slice(b" EI q 1 0 0 1 0 0 cm");
        assert_eq!(super::data_length(&content).unwrap(), data.len());
    }
}

#[test]
fn arithmetic_and_twelve_bit_mutations_remain_bounded() {
    let limits = Limits {
        pixels: 4096,
        decoded_bytes: 1 << 20,
        operations: 100_000,
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
fn arithmetic_work_and_coefficient_storage_use_existing_limits() {
    let data = fixture("image", "jpeg-arithmetic-sequential.in");
    for limits in [
        Limits {
            operations: 100,
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
fn arithmetic_and_huffman_twins_share_a_pixel_scaled_work_budget() {
    // Default Limits admit 24 MP: keep the same operations/pixel ratio here.
    let operations = u64::try_from(Limits::default().operations).unwrap() * (128 * 64) / 24_000_000;
    let limits = Limits {
        operations: usize::try_from(operations).unwrap(),
        ..Limits::default()
    };
    let huffman = fixture("image", "jpeg-budget-huffman.in");
    let arithmetic = fixture("image", "jpeg-budget-arithmetic.in");
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
                operations: 128 * 64 - 1,
                ..limits
            },
        )
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit);
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
    // Six 8x8 blocks: the u16 planes the scan reconstructs, and the
    // expanded u16 samples plus the final bytes through conversion, beside
    // the conversion's rows.
    let bytes = 24 * 16 * (2 + 2 + 1) + 24 * super::transform::ROW_BYTES;
    let limits = Limits {
        decoded_bytes: bytes,
        ..Limits::default()
    };
    assert!(super::decode(&data, Some(0), &limits).is_ok());
    let error = super::decode(
        &data,
        Some(0),
        &Limits {
            decoded_bytes: bytes - 1,
            ..limits
        },
    )
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
}

#[test]
fn sequential_frames_budget_their_sample_planes_not_coefficients() {
    // 1000×1000 without subsampling: 125×125 blocks of 64 samples per
    // component beside the expanded samples and the conversion's rows.
    let header = |precision: u8| {
        let mut data = vec![precision, 0x03, 0xe8, 0x03, 0xe8, 3];
        for id in 1..=3 {
            data.extend([id, 0x11, 0]);
        }
        data
    };
    let pixels = 1000 * 1000 * 3;
    let rows = 1000 * super::transform::ROW_BYTES;
    let planes = 3 * 125 * 125 * 64;
    for (precision, process, bytes) in [
        (8, 0xc0, pixels + planes + rows),
        (8, 0xc1, pixels + planes + rows),
        (12, 0xc1, 3 * pixels + 2 * planes + rows),
        (8, 0xc2, pixels + 5 * planes + rows),
        (12, 0xc2, 3 * pixels + 6 * planes + rows),
    ] {
        let limits = |decoded_bytes| Limits {
            decoded_bytes,
            ..Limits::default()
        };
        let data = header(precision);
        assert!(
            super::Frame::header(&data, process, None, &limits(bytes)).is_ok(),
            "{precision}-bit {process:#x}"
        );
        let Err(error) = super::Frame::header(&data, process, None, &limits(bytes - 1)) else {
            panic!("{precision}-bit {process:#x} admitted below its planes and samples");
        };
        assert_eq!(error.kind, ErrorKind::Limit);
        assert_eq!(error.message, "JPEG coefficient and sample budget exceeded");
    }
}
