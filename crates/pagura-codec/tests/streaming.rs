//! Public incremental PNG controls over independently authored pixel sequences.

use pagura_codec::flate::Level;
use pagura_codec::png::{self, ColorType, Encoder, EncodingLimits, EncodingOutput, Options};
use pagura_core::ErrorKind;

fn pixels(length: usize) -> Vec<u8> {
    let mut state = 0x1425_7593_u32;
    (0..length)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state.to_le_bytes()[0]
        })
        .collect()
}

fn streamed(input: &[u8], dimensions: (u32, u32), options: Options, split: usize) -> Vec<u8> {
    let mut encoder = Encoder::new(
        dimensions.0,
        dimensions.1,
        options.color_type,
        options.level,
        EncodingLimits::default(),
    )
    .unwrap();
    let admitted = encoder.retained_bytes();
    let (mut position, mut bytes) = (0, Vec::new());
    loop {
        match encoder.advance().unwrap() {
            EncodingOutput::Chunk(chunk) => bytes.extend_from_slice(chunk.bytes()),
            EncodingOutput::NeedInput => {
                let end = position.saturating_add(split).min(input.len());
                let count = encoder.write(&input[position..end]).unwrap();
                assert!(
                    count > 0,
                    "encoder requested unavailable input at {position}"
                );
                position += count;
                assert_eq!(encoder.retained_bytes(), admitted);
            }
            EncodingOutput::Yielded => {}
            EncodingOutput::Complete => break,
            other => panic!("unexpected result: {other:?}"),
        }
    }
    assert_eq!(position, input.len());
    assert_eq!(encoder.retained_bytes(), 0);
    assert!(matches!(
        encoder.advance().unwrap(),
        EncodingOutput::Complete
    ));
    encoder.cancel();
    assert!(matches!(
        encoder.advance().unwrap(),
        EncodingOutput::Complete
    ));
    bytes
}

#[test]
fn input_partitions_never_change_png_blocks_filters_or_chunk_boundaries() {
    for (color_type, channels) in [
        (ColorType::Gray, 1),
        (ColorType::Rgb, 3),
        (ColorType::Rgba, 4),
    ] {
        for dimensions in [(1, 1), (7, 13), (257, 273)] {
            let size = usize::try_from(dimensions.0 * dimensions.1).unwrap() * channels;
            let samples = pixels(size);
            for level in [Level::Fast, Level::Default] {
                let options = Options {
                    color_type,
                    level,
                    ..Options::default()
                };
                // The compatibility collector is independently decoded by the
                // existing codec fixture gate; all partition schedules must match it.
                let expected = png::encode(&samples, dimensions.0, dimensions.1, &options).unwrap();
                for split in [1, 17, 257, 65535, usize::MAX] {
                    assert_eq!(streamed(&samples, dimensions, options, split), expected);
                }
            }
        }
    }
}

#[test]
fn output_leases_backpressure_and_survive_terminal_cancellation() {
    let limits = EncodingLimits {
        output_slots: 1,
        ..EncodingLimits::default()
    };
    let mut encoder = Encoder::new(2, 2, ColorType::Rgba, Level::Default, limits).unwrap();
    let EncodingOutput::Chunk(header) = encoder.advance().unwrap() else {
        panic!("PNG header")
    };
    assert_eq!(&header.bytes()[..8], b"\x89PNG\r\n\x1a\n");
    let clone = header.clone();
    let original = header.bytes().to_vec();
    let admitted = encoder.retained_bytes();
    for _ in 0..3 {
        assert!(matches!(
            encoder.advance().unwrap(),
            EncodingOutput::Backpressured
        ));
        assert_eq!(encoder.write(&[0; 16]).unwrap(), 0);
        assert_eq!(encoder.retained_bytes(), admitted);
    }
    drop(header);
    assert!(matches!(
        encoder.advance().unwrap(),
        EncodingOutput::Backpressured
    ));
    encoder.cancel();
    encoder.cancel();
    assert_eq!(encoder.retained_bytes(), clone.retained_bytes());
    assert!(matches!(
        encoder.advance().unwrap(),
        EncodingOutput::Cancelled
    ));
    assert_eq!(clone.bytes(), original);
    assert_eq!(
        encoder.write(&[]).unwrap_err().kind,
        ErrorKind::InvalidArgument
    );
    drop(clone);
    assert_eq!(encoder.retained_bytes(), 0);
}

#[test]
fn partial_final_row_waits_and_excess_input_becomes_terminal_without_iend() {
    let mut encoder = Encoder::new(
        2,
        1,
        ColorType::Rgba,
        Level::Fast,
        EncodingLimits::default(),
    )
    .unwrap();
    drop(encoder.advance().unwrap());
    assert_eq!(encoder.write(&[1, 2, 3]).unwrap(), 3);
    for _ in 0..3 {
        assert!(matches!(
            encoder.advance().unwrap(),
            EncodingOutput::NeedInput
        ));
    }
    assert_eq!(
        encoder.write(&[0; 6]).unwrap_err().kind,
        ErrorKind::InvalidArgument
    );
    assert_eq!(
        encoder.advance().unwrap_err().kind,
        ErrorKind::InvalidArgument
    );
    assert_eq!(encoder.retained_bytes(), 0);
}

#[test]
fn working_storage_stays_bounded_as_image_height_grows() {
    let first = Encoder::new(
        8,
        100_000,
        ColorType::Rgba,
        Level::Default,
        EncodingLimits::default(),
    )
    .unwrap();
    let second = Encoder::new(
        8,
        200_000,
        ColorType::Rgba,
        Level::Default,
        EncodingLimits::default(),
    )
    .unwrap();
    assert_eq!(first.retained_bytes(), second.retained_bytes());
    assert!(first.retained_bytes() < 2 * 1024 * 1024);
}

#[test]
fn streaming_admission_checks_conservative_output_memory_and_work_before_headers() {
    for limits in [
        EncodingLimits {
            output_bytes: 0,
            ..EncodingLimits::default()
        },
        EncodingLimits {
            retained_bytes: 0,
            ..EncodingLimits::default()
        },
        EncodingLimits {
            work: 0,
            ..EncodingLimits::default()
        },
        EncodingLimits {
            pixels: 0,
            ..EncodingLimits::default()
        },
    ] {
        assert_eq!(
            Encoder::new(1, 1, ColorType::Gray, Level::Default, limits)
                .unwrap_err()
                .kind,
            ErrorKind::Limit
        );
    }
    assert_eq!(
        Encoder::new(
            1,
            1,
            ColorType::Gray,
            Level::Default,
            EncodingLimits {
                output_slots: 0,
                ..EncodingLimits::default()
            }
        )
        .unwrap_err()
        .kind,
        ErrorKind::InvalidArgument
    );
    let samples = vec![0; 10_000];
    let encoded = png::encode(
        &samples,
        100,
        100,
        &Options {
            color_type: ColorType::Gray,
            ..Options::default()
        },
    )
    .unwrap();
    assert!(encoded.len() < 1_000);
    let limits = EncodingLimits {
        output_bytes: encoded.len(),
        ..EncodingLimits::default()
    };
    assert_eq!(
        Encoder::new(100, 100, ColorType::Gray, Level::Default, limits)
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
}
