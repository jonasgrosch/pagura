//! Fragment masks are arbitrary rows, unlike homogeneous Exact spans.
use super::*;

fn output(buffer: &Buffer, count: usize) -> Vec<u8> {
    let mut bytes = vec![0; count * 4];
    buffer.output(0, &mut bytes);
    bytes
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
fn both_fragment_sinks_apply_product_then_fractional_clip() {
    // The clip product is rounded first; the half-width final column is
    // rounded second. Both operations use the declared coverage-byte model.
    let mask = [255, 128, 0, 255];
    let product = [128, 255, 255, 255];
    for format in [Format::U8, Format::N16] {
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
