//! Entropy-process resource and malformed-input regressions.
use pagura_core::{ErrorKind, Limits, Position};
use pagura_testkit::fixture;

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
    let limits = Limits {
        operations: Limits::default().operations * (128 * 64) / 24_000_000,
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
    // Six 8x8 blocks: i32 coefficients, u16 planes and expanded samples,
    // plus the final bytes coexist through conversion.
    let bytes = 24 * 16 * (4 + 2 + 2 + 1);
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
