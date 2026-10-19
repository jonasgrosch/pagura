//! Decoder paths the margins select: fast-loop copies of every short
//! period, second-level tables, output growth and the predictor window.
use super::{DIST_BASE, LENGTH_BASE, Level, compress};
use crate::output::paeth;
use crate::{Filter, Predictor, PredictorKind};
use pagura_core::{Diagnostics, ErrorKind, Limits, Position, Recovery};

enum Symbol {
    Literal(u8),
    Match(usize, usize),
}

/// A zlib stream of one fixed-Huffman block (RFC 1951 3.2.6), recording
/// where each symbol ends, as decoder errors report it.
struct Writer {
    bytes: Vec<u8>,
    bit: usize,
    ends: Vec<usize>,
}

impl Writer {
    fn new() -> Self {
        let mut writer = Self {
            bytes: vec![0x78, 0x01],
            bit: 16,
            ends: Vec::new(),
        };
        writer.put(0b011, 3); // final block, fixed codes
        writer
    }

    /// A stored block of `len` zero bytes, then the fixed-Huffman final block.
    fn after_stored(len: usize) -> Self {
        let size = u16::try_from(len).unwrap();
        let mut bytes = vec![0x78, 0x01, 0b000];
        bytes.extend_from_slice(&size.to_le_bytes());
        bytes.extend_from_slice(&(!size).to_le_bytes());
        bytes.resize(bytes.len() + len, 0);
        let mut writer = Self {
            bit: bytes.len() * 8,
            bytes,
            ends: Vec::new(),
        };
        writer.put(0b011, 3); // final block, fixed codes
        writer
    }

    fn put(&mut self, value: usize, width: usize) {
        for index in 0..width {
            if self.bit.is_multiple_of(8) {
                self.bytes.push(0);
            }
            if value >> index & 1 != 0 {
                *self.bytes.last_mut().unwrap() |= 1 << (self.bit % 8);
            }
            self.bit += 1;
        }
    }

    /// A Huffman code, most significant bit first.
    fn code(&mut self, code: usize, width: usize) {
        for index in (0..width).rev() {
            self.put(code >> index & 1, 1);
        }
    }

    fn litlen(&mut self, symbol: usize) {
        match symbol {
            0..=143 => self.code(0x30 + symbol, 8),
            144..=255 => self.code(0x190 + symbol - 144, 9),
            256..=279 => self.code(symbol - 256, 7),
            _ => self.code(0xc0 + symbol - 280, 8),
        }
    }

    fn symbol(&mut self, symbol: &Symbol) {
        match *symbol {
            Symbol::Literal(byte) => self.litlen(usize::from(byte)),
            Symbol::Match(length, distance) => {
                let code = if length == 258 {
                    28
                } else {
                    LENGTH_BASE
                        .iter()
                        .rposition(|&base| base <= length)
                        .unwrap()
                };
                self.litlen(257 + code);
                self.put(
                    length - LENGTH_BASE[code],
                    usize::from(super::LENGTH_EXTRA[code]),
                );
                let code = DIST_BASE
                    .iter()
                    .rposition(|&base| base <= distance)
                    .unwrap();
                self.code(code, 5);
                self.put(
                    distance - DIST_BASE[code],
                    usize::from(super::DIST_EXTRA[code]),
                );
            }
        }
        self.ends.push(self.bit / 8);
    }

    fn finish(mut self, data: &[u8]) -> (Vec<u8>, Vec<usize>) {
        self.litlen(256);
        let (mut a, mut b) = (1_u32, 0_u32);
        for &byte in data {
            a = (a + u32::from(byte)) % 65521;
            b = (b + a) % 65521;
        }
        self.bytes.extend_from_slice(&((b << 16) | a).to_be_bytes());
        (self.bytes, self.ends)
    }
}

fn expand(symbols: &[Symbol]) -> Vec<u8> {
    let mut out = Vec::new();
    for symbol in symbols {
        match *symbol {
            Symbol::Literal(byte) => out.push(byte),
            Symbol::Match(length, distance) => {
                for _ in 0..length {
                    out.push(out[out.len() - distance]);
                }
            }
        }
    }
    out
}

fn encode(symbols: &[Symbol]) -> (Vec<u8>, Vec<u8>, Vec<usize>) {
    let data = expand(symbols);
    let mut writer = Writer::new();
    for symbol in symbols {
        writer.symbol(symbol);
    }
    let (stream, ends) = writer.finish(&data);
    (data, stream, ends)
}

fn decode(input: &[u8], cap: usize, predictor: Predictor) -> pagura_core::Result<Vec<u8>> {
    let mut diagnostics = Diagnostics::new(Recovery::Strict);
    Filter::Flate { predictor }.decode(input, cap, &mut diagnostics)
}

fn noise(seed: &mut u32) -> u8 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 17;
    *seed ^= *seed << 5;
    seed.to_le_bytes()[1]
}

/// Back-references at every distance up to 40 and at chunk-boundary lengths,
/// with the output capacity `Output`'s growth rule gives for this sequence.
#[test]
fn every_short_period_and_length_copies_exactly() {
    let mut seed = 0x1234_5678;
    let mut symbols: Vec<Symbol> = (0..300)
        .map(|_| Symbol::Literal(noise(&mut seed)))
        .collect();
    let lengths = [
        3, 4, 7, 8, 9, 15, 16, 17, 31, 32, 33, 64, 100, 227, 257, 258,
    ];
    for distance in (1..=40).chain([63, 64, 65, 255, 1000, 32768]) {
        for &length in &lengths {
            symbols.push(Symbol::Match(length, distance));
            symbols.push(Symbol::Literal(noise(&mut seed)));
        }
        for _ in 0..100 {
            symbols.push(Symbol::Literal(noise(&mut seed)));
        }
    }
    let (data, stream, _) = encode(&symbols);
    assert!(data.len() > 32768 + 300);
    let decoded = decode(&stream, usize::MAX, Predictor::default()).unwrap();
    assert_eq!(decoded, data);
    let mut capacity = 0_usize;
    let mut len = 0;
    for symbol in &symbols {
        let count = match *symbol {
            Symbol::Literal(_) => 1,
            Symbol::Match(length, _) => length,
        };
        if len + count > capacity {
            capacity = (len + count).max((capacity * 2).max(64));
        }
        len += count;
    }
    assert_eq!(decoded.capacity(), capacity);
    let short = decode(&stream, data.len() - 1, Predictor::default()).unwrap_err();
    assert_eq!(short.kind, ErrorKind::Limit);
}

/// Skewed frequencies give 15-bit codes, which resolve through
/// second-level tables.
#[test]
fn long_codes_decode_through_second_level_tables() {
    let mut weights = vec![1_usize, 1];
    while weights.len() < 26 {
        weights.push(weights[weights.len() - 1] + weights[weights.len() - 2]);
    }
    let mut seed = 0x9e37_79b9;
    let mut data = Vec::new();
    while data.len() < 400_000 {
        let pick = usize::from(noise(&mut seed)) << 8 | usize::from(noise(&mut seed));
        let mut target = pick % weights.iter().sum::<usize>();
        let symbol = weights.iter().position(|&weight| {
            let hit = target < weight;
            target = target.saturating_sub(weight);
            hit
        });
        data.push(b'A' + u8::try_from(symbol.unwrap()).unwrap());
    }
    let stream = compress(&data, Level::Default, &Limits::default()).unwrap();
    assert_eq!(
        decode(&stream, usize::MAX, Predictor::default()).unwrap(),
        data
    );
}

fn png_rows(data: &[u8], row: usize) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    for (index, line) in data.chunks(row + 1).enumerate() {
        let filter = line[0];
        for (column, &byte) in line[1..].iter().enumerate() {
            let left = if column >= 3 { out[out.len() - 3] } else { 0 };
            let above = if index > 0 { out[out.len() - row] } else { 0 };
            let corner = if index > 0 && column >= 3 {
                out[out.len() - row - 3]
            } else {
                0
            };
            let prediction = match filter {
                1 => left,
                2 => above,
                3 => left.midpoint(above),
                4 => paeth(left, above, corner),
                _ => 0,
            };
            out.push(byte.wrapping_add(prediction));
        }
    }
    out
}

/// PNG prediction over a window that slides several times, with matches
/// reaching back across each slide, and an invalid row tag deep inside a
/// fast run reported at its own symbol.
#[test]
fn predictor_window_slides_and_tags_fail_at_their_symbol() {
    let row = 3 * 100;
    let mut seed = 0x0bad_cafe;
    let mut symbols = Vec::new();
    let mut produced = 0;
    let mut tag_symbols = Vec::new();
    while produced < 600_000 {
        tag_symbols.push(symbols.len());
        symbols.push(Symbol::Literal(noise(&mut seed) % 5));
        let mut column = 0;
        while column < row {
            let remaining = row - column;
            if produced > 40_000 && remaining >= 3 && noise(&mut seed) < 160 {
                let length = (3 + usize::from(noise(&mut seed))).min(remaining).min(258);
                let distance = [
                    row + 1,
                    2 * (row + 1),
                    32768,
                    1 + usize::from(noise(&mut seed)),
                ][usize::from(noise(&mut seed) % 4)];
                symbols.push(Symbol::Match(length, distance));
                column += length;
            } else {
                symbols.push(Symbol::Literal(noise(&mut seed)));
                column += 1;
            }
        }
        produced += row + 1;
    }
    // Matches may copy tag bytes into row positions only; tags stay literal.
    let (data, stream, ends) = encode(&symbols);
    let predictor = Predictor {
        kind: PredictorKind::Png,
        colors: 3,
        bits_per_component: 8,
        columns: 100,
    };
    let decoded = decode(&stream, usize::MAX, predictor).unwrap();
    assert_eq!(decoded, png_rows(&data, row));

    let bad = tag_symbols[tag_symbols.len() * 3 / 4];
    symbols[bad] = Symbol::Literal(7);
    let (_, stream, ends_bad) = encode(&symbols);
    assert_eq!(ends, ends_bad);
    let error = decode(&stream, usize::MAX, predictor).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Syntax);
    assert_eq!(error.message, "invalid PNG predictor tag");
    assert_eq!(error.position, Position::Buffer(ends[bad]));
}

#[test]
fn predictor_window_grows_for_a_careful_match_after_a_fast_run() {
    // 16,383 input bytes size the first window just below a power-of-two
    // boundary, so its doublings stop short of the cap; a careful 258-byte
    // match after a fast run must still be able to grow it.
    let zeros = 15_460;
    let matches = 560;
    let mut writer = Writer::after_stored(zeros);
    for _ in 0..matches {
        writer.symbol(&Symbol::Match(258, 1));
    }
    let data = vec![0; zeros + matches * 258];
    let (stream, _) = writer.finish(&data);
    assert_eq!(stream.len(), 16_383);
    let predictor = Predictor {
        kind: PredictorKind::Png,
        colors: 1,
        bits_per_component: 8,
        columns: 1,
    };
    assert_eq!(
        decode(&stream, usize::MAX, predictor).unwrap(),
        vec![0; data.len() / 2]
    );
}
