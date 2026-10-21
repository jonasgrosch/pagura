//! Fragment masks are arbitrary rows, unlike homogeneous Exact spans.
use super::*;

#[test]
fn word_output_matches_rational_rounding_for_every_word() {
    for word in 0..=u16::MAX {
        assert_eq!(u32::from(word_byte(word)), (u32::from(word) + 128) / 257);
    }
    // Distinct channels, a nonzero source offset, complete-pixel truncation,
    // and a destination longer than the remaining source.
    let buffer = Buffer::N16(Storage {
        pixels: vec![
            [0, 128, 129, 65535],
            [256, 257, 32767, 32768],
            [65407, 65408, 65534, 65535],
        ],
        // General dense output must ignore this empty page-output interval.
        painted: 0..0,
    });
    for count in 0..=12 {
        let mut target = vec![77; count];
        buffer.output(1, &mut target);
        let expected = [1, 1, 127, 128, 255, 255, 255, 255];
        let written = (count / 4 * 4).min(expected.len());
        assert_eq!(&target[..written], &expected[..written]);
        assert!(target[written..].iter().all(|&v| v == 77));
    }
}

fn output(buffer: &Buffer, count: usize) -> Vec<u8> {
    let mut bytes = vec![0; count * 4];
    buffer.output(0, &mut bytes);
    bytes
}

#[test]
fn float_dense_output_ignores_the_page_interval_and_keeps_partial_pixels() {
    let buffer = Buffer::F32(Storage {
        pixels: vec![
            [0.0, 0.5, 1.5, 255.0],
            [17.49, 85.5, 170.5, 255.0],
            [f32::NEG_INFINITY, f32::INFINITY, f32::NAN, -1.0],
        ],
        // A page-only optimization must not affect this dense API.
        painted: 0..0,
    });
    for count in 0..=12 {
        let mut target = vec![77; count];
        buffer.output(1, &mut target);
        let expected = [17, 86, 171, 255, 0, 255, 0, 0];
        let written = (count / 4 * 4).min(expected.len());
        assert_eq!(&target[..written], &expected[..written]);
        assert!(target[written..].iter().all(|&v| v == 77));
    }
    let mut tail = [77; 4];
    buffer.output(3, &mut tail);
    assert_eq!(tail, [77; 4]);
}

#[test]
fn dynamic_preparation_preserves_pixels_storage_and_validation() {
    let background = [17, 85, 170, 255];
    let mut floats = Buffer::new(Format::F32, 4, background).unwrap();
    // Index validation precedes the debug-only prepared-operation check.
    let error = floats
        .paint_dynamic(4, Source::new([0.0; 4], 0.5, 1.0))
        .unwrap_err();
    assert_eq!(error.message, "source outside colour buffer");
    assert!(floats.painted_range().unwrap().is_empty());
    floats.prepare_dynamic();
    assert_eq!(floats.painted_range(), Some(0..4));
    assert_eq!(output(&floats, 4), background.repeat(4));
    assert_eq!(floats.bytes(), 64);
    floats
        .paint_dynamic(2, Source::new([0.0; 4], 0.5, 1.0))
        .unwrap();
    floats.prepare_dynamic();
    let mut expected = background.repeat(4);
    expected[8..12].copy_from_slice(&[9, 43, 85, 255]);
    assert_eq!(output(&floats, 4), expected);
    assert_eq!(floats.bytes(), 64);

    let mut empty = Buffer::new(Format::F32, 0, background).unwrap();
    empty.prepare_dynamic();
    assert_eq!(empty.painted_range(), Some(0..0));
    let error = empty
        .paint_dynamic(0, Source::new([0.0; 4], 1.0, 1.0))
        .unwrap_err();
    assert_eq!(error.message, "source outside colour buffer");
    assert_eq!(empty.bytes(), 0);

    for format in [Format::U8, Format::N16] {
        let mut buffer = Buffer::new(format, 4, background).unwrap();
        buffer.path(2, &[255], [0.0; 4], 1.0).unwrap();
        let painted = buffer.painted_range();
        let before = output(&buffer, 4);
        let bytes = buffer.bytes();
        buffer.prepare_dynamic();
        assert_eq!(buffer.painted_range(), painted);
        assert_eq!(output(&buffer, 4), before);
        assert_eq!(buffer.bytes(), bytes);
        if format == Format::N16 {
            let error = buffer
                .paint_dynamic(0, Source::new([0.0; 4], 1.0, 1.0))
                .unwrap_err();
            assert_eq!(error.message, "dynamic source in N16 buffer");
        }
    }
}

#[test]
fn narrow_painted_interval_tracks_validated_spans_conservatively() {
    for (format, bytes) in [(Format::N16, 64), (Format::F32, 128)] {
        let mut buffer = Buffer::new(format, 8, [255; 4]).unwrap();
        assert!(buffer.painted_range().unwrap().is_empty());
        assert_eq!(buffer.bytes(), bytes);
        buffer.path(8, &[], [0.0; 4], 1.0).unwrap();
        assert!(buffer.painted_range().unwrap().is_empty());
        let error = buffer.path(9, &[], [0.0; 4], 1.0).unwrap_err();
        assert_eq!(error.message, "mask outside colour buffer");
        assert!(buffer.painted_range().unwrap().is_empty());

        // No coverage scan decides whether the conservative interval widens.
        buffer.path(2, &[0, 1], [0.0; 4], 1.0).unwrap();
        assert_eq!(buffer.painted_range(), Some(2..4));
        let error = buffer.path(7, &[255; 2], [1.0; 4], 1.0).unwrap_err();
        assert_eq!(error.message, "mask outside colour buffer");
        assert_eq!(buffer.painted_range(), Some(2..4));
        buffer.path(7, &[255], [0.0; 4], 1.0).unwrap();
        assert_eq!(buffer.painted_range(), Some(2..8));
        buffer.path(0, &[255], [1.0; 4], 1.0).unwrap();
        assert_eq!(buffer.painted_range(), Some(0..8));
        let expected: Vec<u8> = [255, 255, 255, 254, 255, 255, 255, 0]
            .into_iter()
            .flat_map(|gray| [gray, gray, gray, 255])
            .collect();
        assert_eq!(output(&buffer, 8), expected);
        assert_eq!(buffer.bytes(), bytes);
    }
}

#[test]
fn narrow_fragment_interval_keeps_prior_rows_when_a_later_clip_row_fails() {
    for (format, bytes) in [(Format::N16, 96), (Format::F32, 192)] {
        let mut buffer = Buffer::new(format, 12, [255; 4]).unwrap();
        let error = buffer
            .mask_sink((4, 3), [0.0; 4], 1.0)
            .unwrap()
            .mask(
                (&[255; 4], 2, (-2, 7)),
                ((-3, 6), [-3, 6, 1, 9]),
                MaskClip {
                    // The first product row is valid; the second is missing.
                    product: Some((&[255, 128], [-2, 7, 0, 9])),
                    rectangle: None,
                },
                &mut Vec::new(),
            )
            .unwrap_err();
        assert_eq!(error.message, "mask exceeds clip");
        assert_eq!(buffer.painted_range(), Some(5..7));
        let mut expected = vec![255; 48];
        expected[5 * 4..6 * 4].copy_from_slice(&[0, 0, 0, 255]);
        expected[6 * 4..7 * 4].copy_from_slice(&[127, 127, 127, 255]);
        assert_eq!(output(&buffer, 12), expected);
        assert_eq!(buffer.bytes(), bytes);

        let mut empty = Buffer::new(format, 4, [255; 4]).unwrap();
        empty
            .mask_sink((4, 1), [0.0; 4], 1.0)
            .unwrap()
            .mask(
                (&[255; 4], 4, (10, 0)),
                ((0, 0), [0, 0, 4, 1]),
                MaskClip::default(),
                &mut Vec::new(),
            )
            .unwrap();
        assert!(empty.painted_range().unwrap().is_empty());
        assert_eq!(output(&empty, 4), [255; 16]);
    }
}

#[test]
fn narrow_distant_fragment_rows_keep_the_holes_and_full_interval() {
    let background = [17, 85, 170, 255];
    for (format, bytes) in [(Format::N16, 96), (Format::F32, 192)] {
        let mut buffer = Buffer::new(format, 12, background).unwrap();
        for origin in [(10, -3), (13, -1)] {
            buffer
                .mask_sink((4, 3), [0.0; 4], 1.0)
                .unwrap()
                .mask(
                    (&[255], 1, origin),
                    ((10, -3), [10, -3, 14, 0]),
                    MaskClip::default(),
                    &mut Vec::new(),
                )
                .unwrap();
        }
        assert_eq!(buffer.painted_range(), Some(0..12));
        let mut expected = background.repeat(12);
        expected[..4].copy_from_slice(&[0, 0, 0, 255]);
        expected[44..].copy_from_slice(&[0, 0, 0, 255]);
        assert_eq!(output(&buffer, 12), expected);
        assert_eq!(buffer.bytes(), bytes);
    }
}

#[test]
fn mixed_fragment_rows_preserve_each_coverage_and_device_origin() {
    let mut buffer = Buffer::new(Format::N16, 8, [255; 4]).unwrap();
    let mask = [255, 128, 0, 255, 0, 1, 254, 255];
    buffer
        .mask_sink((4, 2), [0.0; 4], 1.0)
        .unwrap()
        .mask(
            (&mask, 4, (-2, 7)),
            ((-2, 7), [-2, 7, 2, 9]),
            MaskClip::default(),
            &mut Vec::new(),
        )
        .unwrap();
    let expected: Vec<u8> = mask
        .into_iter()
        .flat_map(|coverage| [255 - coverage, 255 - coverage, 255 - coverage, 255])
        .collect();
    assert_eq!(output(&buffer, 8), expected);
}

#[test]
fn fragment_sinks_apply_product_then_fractional_clip() {
    // The clip product is rounded first; the half-width final column is
    // rounded second. Both operations use the declared coverage-byte model.
    let mask = [255, 128, 0, 255];
    let product = [128, 255, 255, 255];
    for format in [Format::U8, Format::N16, Format::F32] {
        let mut buffer = Buffer::new(format, 4, [255; 4]).unwrap();
        buffer
            .mask_sink((4, 1), [0.0; 4], 1.0)
            .unwrap()
            .mask(
                (&mask, 4, (10, -3)),
                ((10, -3), [10, -3, 14, -2]),
                MaskClip {
                    product: Some((&product, [10, -3, 14, -2])),
                    rectangle: Some([10.0, -3.0, 13.5, -2.0]),
                },
                &mut Vec::new(),
            )
            .unwrap();
        assert_eq!(
            output(&buffer, 4),
            [
                127, 127, 127, 255, 127, 127, 127, 255, 255, 255, 255, 255, 127, 127, 127, 255
            ]
        );
        if format == Format::U8 {
            assert!(buffer.painted_range().is_none());
        } else {
            assert_eq!(buffer.painted_range(), Some(0..4));
        }
    }
}

#[test]
fn n16_fragment_sources_and_long_stacks_keep_independent_precision() {
    let area = ((0, 0), [0, 0, 1, 1]);
    let mut buffer = Buffer::new(Format::N16, 1, [255; 4]).unwrap();
    let mut scratch = Vec::new();
    buffer
        .mask_sink((1, 1), [0.85; 4], 1.0)
        .unwrap()
        .mask((&[255], 1, (0, 0)), area, MaskClip::default(), &mut scratch)
        .unwrap();
    let mut sink = buffer.mask_sink((1, 1), [0.95; 4], 1.0).unwrap();
    for _ in 0..2 {
        sink.mask((&[10], 1, (0, 0)), area, MaskClip::default(), &mut scratch)
            .unwrap();
    }
    // 44617/204 = 218.71078...; no early RGB8 source quantization.
    let pixel = output(&buffer, 1);
    assert!(pixel[..3].iter().all(|v| (218..=219).contains(v)));
    assert_eq!(pixel[3], 255);

    let mut buffer = Buffer::new(Format::N16, 1, [255; 4]).unwrap();
    let mut sink = buffer.mask_sink((1, 1), [0.0; 4], 1.0).unwrap();
    for _ in 0..1000 {
        sink.mask((&[1], 1, (0, 0)), area, MaskClip::default(), &mut scratch)
            .unwrap();
    }
    // 255*(254/255)^1000 = 5.01275...; bytes alone stagnate at127.
    let pixel = output(&buffer, 1);
    assert!(pixel[..3].iter().all(|v| (5..=6).contains(v)));
    assert_eq!(pixel[3], 255);
}

#[test]
fn u8_fragment_windows_and_n16_admission_keep_their_boundaries() {
    let mut bytes = Bytes::new((4, 3), [255; 4], Vec::new()).unwrap();
    bytes.window(1, 1).unwrap();
    let mut buffer = Buffer::U8(bytes);
    buffer
        .mask_sink((4, 1), [0.0; 4], 1.0)
        .unwrap()
        .mask(
            (&[255, 0, 128, 255], 4, (0, 8)),
            ((0, 8), [0, 8, 4, 9]),
            MaskClip::default(),
            &mut Vec::new(),
        )
        .unwrap();
    let Buffer::U8(bytes) = buffer else {
        unreachable!()
    };
    assert_eq!(&bytes.pixels[..16], &[255; 16]);
    assert_eq!(&bytes.pixels[32..], &[255; 16]);
    assert_eq!(bytes.painted, [[u32::MAX, 0], [0, 4], [u32::MAX, 0]]);
    let mut buffer = Buffer::new(Format::N16, 4, [255; 4]).unwrap();
    assert!(buffer.mask_sink((4, 1), [0.0; 4], 0.5).is_err());
    assert!(buffer.mask_sink((3, 1), [0.0; 4], 1.0).is_err());
    let error = buffer
        .mask_sink((4, 1), [0.0; 4], 1.0)
        .unwrap()
        .mask(
            (&[255; 4], 4, (0, 0)),
            ((1, 0), [0, 0, 4, 1]),
            MaskClip::default(),
            &mut Vec::new(),
        )
        .unwrap_err();
    assert_eq!(error.message, "mask outside colour buffer");
}

fn float_bits(buffer: &Buffer) -> Vec<[u32; 4]> {
    let Buffer::F32(floats) = buffer else {
        unreachable!()
    };
    floats
        .pixels
        .iter()
        .map(|pixel| pixel.map(f32::to_bits))
        .collect()
}

#[test]
fn f32_fragment_masks_keep_path_arithmetic_for_every_coverage() {
    // The second mixed row begins at 255: that byte alone cannot establish
    // the homogeneous full-span contract that Buffer::path receives.
    let coverage: Vec<u8> = (0..=255).chain((0..=255).rev()).collect();
    for background in [[255; 4], [17, 93, 211, 255]] {
        for color in [[0.0; 4], [1.0, 0.5, 0.25, 0.0], [0.85, 0.95, 0.1, 0.0]] {
            for alpha in [0.0, 1.0 / 1024.0, 0.5, 254.0 / 255.0, 1.0] {
                let mut masks = Buffer::new(Format::F32, coverage.len(), background).unwrap();
                let mut paths = Buffer::new(Format::F32, coverage.len(), background).unwrap();
                let mut scratch = Vec::new();
                for _ in 0..3 {
                    masks
                        .mask_sink((256, 2), color, alpha)
                        .unwrap()
                        .mask(
                            (&coverage, 256, (-7, 11)),
                            ((-7, 11), [-7, 11, 249, 13]),
                            MaskClip::default(),
                            &mut scratch,
                        )
                        .unwrap();
                    for (index, value) in coverage.iter().enumerate() {
                        paths
                            .path(index, std::slice::from_ref(value), color, alpha)
                            .unwrap();
                    }
                    assert_eq!(float_bits(&masks), float_bits(&paths));
                    assert_eq!(
                        output(&masks, coverage.len()),
                        output(&paths, coverage.len())
                    );
                }
            }
        }
    }
}

#[test]
fn f32_fragment_offsets_and_clip_order_keep_path_bits() {
    // The first mask row and first column lie outside the surface. Product
    // coverage is rounded first, then fractional rectangle coverage. Literal
    // final bytes below independently specify those two integer roundings.
    let mask = [
        255, 255, 255, 255, 255, 0, 17, 255, 128, 64, 1, 2, 0, 255, 200,
    ];
    let product = [128, 255, 64, 255, 255, 128, 255, 255, 255, 255, 255, 255];
    let coverage = [
        0, 32, 64, 4, 0, 0, 0, 0, 128, 100, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ];
    for alpha in [0.5, 1.0 / 1024.0, 1.0] {
        let color = [0.85, 0.95, 0.1, 0.0];
        let mut masks = Buffer::new(Format::F32, 24, [17, 93, 211, 255]).unwrap();
        let mut paths = Buffer::new(Format::F32, 24, [17, 93, 211, 255]).unwrap();
        masks
            .mask_sink((6, 4), color, alpha)
            .unwrap()
            .mask(
                (&mask, 5, (9, -4)),
                ((10, -3), [10, -3, 16, 1]),
                MaskClip {
                    product: Some((&product, [11, -3, 15, 0])),
                    rectangle: Some([11.5, -2.5, 13.5, -0.5]),
                },
                &mut Vec::new(),
            )
            .unwrap();
        for (index, value) in coverage.iter().enumerate() {
            paths
                .path(index, std::slice::from_ref(value), color, alpha)
                .unwrap();
        }
        assert_eq!(float_bits(&masks), float_bits(&paths));
        assert_eq!(output(&masks, 24), output(&paths, 24));
    }
}

#[test]
fn f32_fragment_admission_and_surface_bounds_are_checked() {
    let mut buffer = Buffer::new(Format::F32, 4, [255; 4]).unwrap();
    for size in [(0, 4), (3, 1), (usize::MAX, 2)] {
        assert!(buffer.mask_sink(size, [0.0; 4], 0.5).is_err());
    }
    let error = buffer
        .mask_sink((4, 1), [0.0; 4], 0.5)
        .unwrap()
        .mask(
            (&[255; 4], 4, (0, 0)),
            ((1, 0), [0, 0, 4, 1]),
            MaskClip::default(),
            &mut Vec::new(),
        )
        .unwrap_err();
    assert_eq!(error.message, "mask outside colour buffer");
    assert_eq!(output(&buffer, 4), [255; 16]);
    assert!(buffer.painted_range().unwrap().is_empty());
}
