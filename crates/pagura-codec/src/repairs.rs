//! Stable, security-relevant [`Repair`] tags for damaged filter input
//! (ADR 0003). Under `Recovery::Strict` each is a syntax error carrying the
//! tag; under `Recovery::Repair` it is reported and decoding continues.
use pagura_core::Repair;

/// FlateDecode data lacks the zlib wrapper; decoded as raw DEFLATE.
/// 7.4.4 requires RFC 1950 framing. Explicit
/// [`decode_raw_deflate`](crate::decode_raw_deflate) does not apply it.
pub const RAW_DEFLATE: Repair = Repair::new("filter.raw-deflate");
/// Keep the decoded prefix when input ends before the required end marker.
/// Incomplete predictor rows remain errors; no synthetic pixels are added.
pub const TRUNCATED: Repair = Repair::new("filter.truncated");
/// Keep a full 12-bit LZW dictionary through entry 4095 without further
/// insertions until clear or EOD; reported once per stream. This recovers an
/// encoder fault: 7.4.4.2 requires a clear code once the table is full.
pub const LZW_FULL_TABLE: Repair = Repair::new("filter.lzw-full-table");
/// Start with the initial literal dictionary when the leading clear code,
/// which 7.4.4.2 requires of the encoder, is absent and the first code is a
/// literal or EOD. Any other first code is still a syntax error.
pub const LZW_INITIAL_CLEAR: Repair = Repair::new("filter.lzw-initial-clear");
/// Ignore nonzero unused bits after LZW EOD, which 7.4.4.2 requires the
/// encoder to zero.
pub const LZW_PADDING: Repair = Repair::new("filter.lzw-padding");
/// Keep decoded bytes despite a mismatched zlib Adler-32 checksum.
pub const CHECKSUM_MISMATCH: Repair = Repair::new("filter.checksum-mismatch");
/// Ignore bytes after the first end marker. ASCII filters permit trailing
/// PDF whitespace; binary filters report any trailing byte, including zero.
pub const TRAILING_DATA: Repair = Repair::new("filter.trailing-data");

/// Every repair tag of this crate.
pub const ALL: &[Repair] = &[
    RAW_DEFLATE,
    TRUNCATED,
    LZW_FULL_TABLE,
    LZW_INITIAL_CLEAR,
    LZW_PADDING,
    CHECKSUM_MISMATCH,
    TRAILING_DATA,
];

#[cfg(test)]
mod tests {
    use super::ALL;

    #[test]
    fn tags_are_unique_and_well_formed() {
        for (index, tag) in ALL.iter().enumerate() {
            let (area, item) = tag.as_str().split_once('.').unwrap();
            assert_eq!(area, "filter", "{tag}");
            assert!(!item.is_empty(), "{tag}");
            assert!(
                item.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'),
                "{tag}"
            );
            assert!(!ALL[..index].contains(tag), "{tag}");
        }
    }
}
