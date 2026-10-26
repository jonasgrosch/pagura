//! Entropy-process resource and malformed-input regressions.
use pagura_core::{ErrorKind, Limits, Position};
use pagura_testkit::fixture;

#[test]
fn arithmetic_and_twelve_bit_mutations_remain_bounded() {
    let limits = Limits {
        pixels: 4096,
        decoded_bytes: 1 << 20,
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
fn arithmetic_work_and_coefficient_storage_use_existing_limits() {
    let data = fixture("image", "jpeg-arithmetic-sequential.in");
    for limits in [
        Limits {
            decode_work: 100,
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
    // Default Limits admit `decode_work / pixels` units per pixel: keep the
    // same ratio for this 128×64 image.
    let defaults = Limits::default();
    let limits = Limits {
        decode_work: defaults.decode_work / defaults.pixels * (128 * 64),
        ..defaults
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
                decode_work: 128 * 64 - 1,
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
    // Six 8x8 blocks: the u16 planes the scan reconstructs and the expanded
    // u16 samples beside the conversion's rows are working memory; the final
    // bytes are the output.
    let working = 24 * 16 * (2 + 2) + 24 * super::transform::ROW_BYTES;
    let output = 24 * 16;
    let limits = Limits {
        decode_bytes: working,
        decoded_bytes: output,
        ..Limits::default()
    };
    assert!(super::decode(&data, Some(0), &limits).is_ok());
    for limits in [
        Limits {
            decode_bytes: working - 1,
            ..limits
        },
        Limits {
            decoded_bytes: output - 1,
            ..limits
        },
    ] {
        let error = super::decode(&data, Some(0), &limits).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit);
    }
}

/// A 1000×1000 frame of three components without subsampling: 125×125
/// blocks of 64 samples each.
fn header(precision: u8) -> Vec<u8> {
    let mut data = vec![precision, 0x03, 0xe8, 0x03, 0xe8, 3];
    for id in 1..=3 {
        data.extend([id, 0x11, 0]);
    }
    data
}

#[test]
fn frames_admit_working_memory_against_decode_bytes_and_samples_against_decoded_bytes() {
    let samples = 1000 * 1000 * 3;
    let rows = 1000 * super::transform::ROW_BYTES;
    let planes = 3 * 125 * 125 * 64;
    for (precision, process, working) in [
        (8, 0xc0, planes + rows),
        (8, 0xc1, planes + rows),
        (12, 0xc1, 2 * samples + 2 * planes + rows),
        (8, 0xc2, 5 * planes + rows),
        (12, 0xc2, 2 * samples + 6 * planes + rows),
    ] {
        let limits = |decode_bytes, decoded_bytes| Limits {
            decode_bytes,
            decoded_bytes,
            ..Limits::default()
        };
        let data = header(precision);
        let case = format!("{precision}-bit {process:#x}");
        assert!(
            super::Frame::header(&data, process, &limits(working, samples)).is_ok(),
            "{case}"
        );
        let Err(error) = super::Frame::header(&data, process, &limits(working - 1, samples)) else {
            panic!("{case} admitted below its working memory");
        };
        assert_eq!(error.message, "JPEG plane and coefficient budget exceeded");
        let Err(error) = super::Frame::header(&data, process, &limits(working, samples - 1)) else {
            panic!("{case} admitted below its samples");
        };
        assert_eq!(error.message, "image sample budget exceeded");
    }
}

#[test]
fn native_planes_are_admitted_against_decode_bytes() {
    // A block flag per 64 samples and a prefix table per component, beside
    // the conversion's rows.
    let need = 3 * 125 * 125 * 65 + 3 * 4096 * 4 + 1000 * super::transform::ROW_BYTES;
    let limits = |decode_bytes| Limits {
        decode_bytes,
        ..Limits::default()
    };
    let native = super::Native { transposed: false };
    let mut frame = super::Frame::header(&header(8), 0xc0, &limits(need)).unwrap();
    super::deferred::prepare(&mut frame, (None, None), &limits(need), native).unwrap();
    let mut frame = super::Frame::header(&header(8), 0xc0, &limits(need)).unwrap();
    let error =
        super::deferred::prepare(&mut frame, (None, None), &limits(need - 1), native).unwrap_err();
    assert_eq!(error.message, "JPEG native plane budget exceeded");
}
