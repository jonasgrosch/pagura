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

/// Exact rational area means of a full decode's samples over cell (row,
/// column) of `cells` per eight, when the cell lies inside the frame.
fn cell_mean(
    full: &super::DctImage,
    cells: u32,
    (row, column): (u32, u32),
    c: usize,
) -> Option<u8> {
    let components = usize::from(full.components);
    let (x0, x1) = (column * 8, (column + 1) * 8);
    let (y0, y1) = (row * 8, (row + 1) * 8);
    if x1 > full.width * cells || y1 > full.height * cells {
        return None;
    }
    // Work in units of 1/cells of a sample: cell edges fall on 8 units.
    let overlap = |s: u32, low: u32, high: u32| {
        (cells * (s + 1))
            .min(high)
            .saturating_sub((cells * s).max(low))
    };
    let mut sum = 0u64;
    for y in 0..full.height {
        let wy = overlap(y, y0, y1);
        for x in 0..full.width {
            let w = u64::from(wy * overlap(x, x0, x1));
            let index = (y as usize * full.width as usize + x as usize) * components + c;
            sum += w * u64::from(full.samples[index]);
        }
    }
    u8::try_from((sum + 32) / 64).ok()
}

#[test]
fn six_cells_stay_within_a_level_of_the_full_gray_sample_means() {
    let limits = Limits::default();
    let mut strict = pagura_core::Diagnostics::new(pagura_core::Recovery::Strict);
    for name in [
        "jpeg-baseline-gray",
        "jpeg-restart",
        "jpeg-progressive",
        "jpeg-extended",
    ] {
        let data = fixture("image", &format!("{name}.in"));
        let full = super::decode(&data, Some(0), &limits).unwrap();
        let six = super::decode_cells(&data, Some(0), &limits, &mut strict, 6).unwrap();
        assert_eq!(six.cells, 6);
        assert_eq!(
            (six.width, six.height),
            (
                super::scaled_length(full.width, 6),
                super::scaled_length(full.height, 6)
            )
        );
        let mut compared = 0;
        for row in 0..six.height {
            for column in 0..six.width {
                if let Some(expected) = cell_mean(&full, 6, (row, column), 0) {
                    // Means of the unclamped samples, rounded once, against
                    // means of the rounded, clamped samples.
                    let actual = six.samples[(row * six.width + column) as usize];
                    assert!(actual.abs_diff(expected) <= 1, "{name} ({row},{column})");
                    compared += 1;
                }
            }
        }
        assert!(compared > 0, "{name}");
    }
}

#[test]
fn reduced_frames_decode_and_fail_as_full_frames_do() {
    let limits = Limits {
        pixels: 4096,
        decoded_bytes: 1 << 20,
        operations: 100_000,
        ..Limits::default()
    };
    let mut repair = pagura_core::Diagnostics::new(pagura_core::Recovery::Repair);
    for name in [
        "jpeg-baseline-gray",
        "jpeg-baseline-rgb",
        "jpeg-420",
        "jpeg-interleaved-420",
        "jpeg-interleaved-progressive",
        "jpeg-progressive-rgb",
        "jpeg-arithmetic-sequential",
        "jpeg-cmyk",
        "jpeg-bad-huffman",
        "jpeg-missing-scan",
        "jpeg-truncated",
    ] {
        let data = fixture("image", &format!("{name}.in"));
        let full = super::decode(&data, None, &limits);
        for cells in [1, 2, 3, 4, 6, 8] {
            let reduced = super::decode_cells(&data, None, &limits, &mut repair, cells);
            match (&full, &reduced) {
                (Ok(full), Ok(reduced)) => {
                    assert_eq!(reduced.cells, cells, "{name}");
                    assert_eq!(reduced.components, full.components, "{name}");
                    assert_eq!(reduced.width, super::scaled_length(full.width, cells));
                    assert_eq!(reduced.height, super::scaled_length(full.height, cells));
                    let count = reduced.width as usize
                        * reduced.height as usize
                        * usize::from(reduced.components);
                    assert_eq!(reduced.samples.len(), count, "{name} {cells}");
                    if cells == 8 {
                        assert_eq!(reduced, full, "{name}");
                    }
                }
                (Err(full), Err(reduced)) => assert_eq!(reduced.kind, full.kind, "{name}"),
                _ => panic!("{name} with {cells} cells: {full:?} but {reduced:?}"),
            }
        }
        for end in (0..data.len()).step_by(3) {
            let prefix = &data[..end];
            assert_eq!(
                super::decode_cells(prefix, None, &limits, &mut repair, 3).is_ok(),
                super::decode(prefix, None, &limits).is_ok(),
                "{name} prefix {end}"
            );
        }
    }
}

#[test]
fn twelve_bit_frames_keep_every_sample_and_counts_are_validated() {
    let limits = Limits::default();
    let mut strict = pagura_core::Diagnostics::new(pagura_core::Recovery::Strict);
    let data = fixture("image", "jpeg-12bit-sequential.in");
    let reduced = super::decode_cells(&data, Some(0), &limits, &mut strict, 2).unwrap();
    assert_eq!(reduced.cells, 8);
    assert_eq!(reduced, super::decode(&data, Some(0), &limits).unwrap());
    for cells in [0, 5, 7, 9, 16] {
        let error = super::decode_cells(&data, Some(0), &limits, &mut strict, cells).unwrap_err();
        assert_eq!(error.kind, ErrorKind::InvalidArgument);
    }
}
