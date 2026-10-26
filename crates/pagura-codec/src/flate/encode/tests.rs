//! Canonical block/history controls independent of input delivery boundaries.

use super::{Compressor, Encoded};
use crate::flate::{Level, compress};
use pagura_core::{Limits, Meter};

fn chunks(input: &[u8], level: Level, split: usize) -> Vec<u8> {
    let mut bytes = Meter::new(2 * 1024 * 1024, "test scratch");
    let mut encoder = Compressor::new(input.len(), level, &mut bytes).unwrap();
    let admitted = bytes.used();
    let (mut position, mut output) = (0_usize, Vec::new());
    loop {
        let end = position.saturating_add(split).min(input.len());
        position += encoder.push(&input[position..end]).unwrap();
        match encoder.advance().unwrap() {
            Encoded::NeedInput => assert!(position < input.len()),
            Encoded::Bytes(data) => output.extend_from_slice(data),
            Encoded::Complete => break,
        }
        assert_eq!(bytes.used(), admitted);
    }
    assert_eq!(position, input.len());
    output
}

#[test]
fn canonical_boundaries_preserve_history_and_two_byte_hash_lookahead() {
    let mut data = vec![0; 2 * 65_535 + 259];
    for (i, byte) in data.iter_mut().enumerate() {
        *byte = i.wrapping_mul(43).wrapping_add(i / 257).to_le_bytes()[0];
    }
    // Repeated history crosses both the retained-window edge and block seam.
    data.copy_within(32_767..32_767 + 258, 65_534);
    for length in [0, 1, 2, 3, 32_768, 65_534, 65_535, 65_536, data.len()] {
        for level in [Level::Fast, Level::Default] {
            let input = &data[..length];
            let expected = compress(input, level, &Limits::default()).unwrap();
            for split in [1, 2, 257, 65_535, usize::MAX] {
                assert_eq!(
                    chunks(input, level, split),
                    expected,
                    "length {length}, split {split}"
                );
            }
        }
    }
}
