//! RFC 1951 constants shared by the decoder and the encoder.

/// How far back a match may reach (RFC 1951 2): 32 KiB.
pub(crate) const WINDOW: usize = 32 * 1024;
/// The longest literal/length or distance code (RFC 1951 3.2.7).
pub(super) const MAX_CODE_BITS: u8 = 15;
/// The longest code-length code: three-bit lengths (RFC 1951 3.2.7).
pub(super) const MAX_CODE_LENGTH_BITS: u8 = 7;
/// The longest stored block (RFC 1951 3.2.4: LEN is 16 bits).
pub(super) const MAX_STORED: usize = 65535;

/// The literal/length symbol that ends a block (RFC 1951 3.2.5).
pub(super) const END_OF_BLOCK: u16 = 256;
/// The first length symbol (RFC 1951 3.2.5).
pub(super) const FIRST_LENGTH: u16 = 257;
/// The last length symbol, for length 258 (RFC 1951 3.2.5).
pub(super) const LAST_LENGTH: u16 = 285;
/// Literal/length symbols a dynamic block may use, 0..=285.
pub(super) const LITERAL_SYMBOLS: usize = 286;
/// Literal/length symbols of the fixed code, including the unused 286 and
/// 287 that take part in its construction (RFC 1951 3.2.6).
pub(super) const FIXED_LITERAL_SYMBOLS: usize = 288;
/// Distance symbols in use, 0..=29 (RFC 1951 3.2.5).
pub(super) const DISTANCE_SYMBOLS: usize = 30;
/// Distance symbols of the fixed code, including the unused 30 and 31.
pub(super) const FIXED_DISTANCE_SYMBOLS: usize = 32;
/// The fixed code's distance code length (RFC 1951 3.2.6).
pub(super) const FIXED_DISTANCE_BITS: u8 = 5;
/// Code-length symbols (RFC 1951 3.2.7).
pub(super) const CODE_LENGTH_SYMBOLS: usize = 19;
/// The fewest code-length code lengths a dynamic header stores (HCLEN + 4).
pub(super) const MIN_CODE_LENGTH_CODES: usize = 4;

/// The order in which a dynamic header stores the code-length code lengths
/// (RFC 1951 3.2.7).
pub(super) const CODE_LENGTH_ORDER: [usize; CODE_LENGTH_SYMBOLS] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

/// Code-length symbol: copy the previous length 3-6 times (2 extra bits).
pub(super) const REPEAT_PREVIOUS: u8 = 16;
/// Code-length symbol: 3-10 zero lengths (3 extra bits).
pub(super) const REPEAT_ZERO: u8 = 17;
/// Code-length symbol: 11-138 zero lengths (7 extra bits).
pub(super) const REPEAT_ZERO_LONG: u8 = 18;

/// The block types of the BTYPE field (RFC 1951 3.2.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(super) enum BlockType {
    Stored = 0,
    Fixed = 1,
    Dynamic = 2,
}

impl BlockType {
    /// The block type of a BTYPE value; 3 is reserved.
    pub(super) const fn from_bits(value: usize) -> Option<Self> {
        match value {
            0 => Some(Self::Stored),
            1 => Some(Self::Fixed),
            2 => Some(Self::Dynamic),
            _ => None,
        }
    }
}

/// The fixed literal/length code lengths (RFC 1951 3.2.6).
pub(super) const fn fixed_literal_lengths() -> [u8; FIXED_LITERAL_SYMBOLS] {
    let mut lengths = [8; FIXED_LITERAL_SYMBOLS];
    let mut symbol = 144;
    while symbol < 280 {
        lengths[symbol] = if symbol < 256 { 9 } else { 7 };
        symbol += 1;
    }
    lengths
}

// RFC 1951 3.2.5, length and distance tables.
pub(super) const LENGTH_BASE: [usize; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
pub(super) const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
pub(super) const DIST_BASE: [usize; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
pub(super) const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
/// The longest match (RFC 1951 3.2.5).
pub(super) const MAX_MATCH: usize = 258;
/// The shortest match.
pub(super) const MIN_MATCH: usize = 3;

#[cfg(test)]
mod tests {
    use super::fixed_literal_lengths;

    #[test]
    fn fixed_literal_lengths_follow_rfc_1951_3_2_6() {
        let lengths = fixed_literal_lengths();
        for (symbol, &length) in lengths.iter().enumerate() {
            let expected = match symbol {
                0..=143 | 280..=287 => 8,
                144..=255 => 9,
                _ => 7,
            };
            assert_eq!(length, expected, "{symbol}");
        }
    }
}
