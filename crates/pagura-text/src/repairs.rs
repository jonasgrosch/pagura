//! Stable tags for reported font repairs (ADR 0003). ToUnicode only controls
//! extraction (9.10.3); malformed maps still require permission to repair or
//! discard.
use pagura_core::Repair;

/// Missing embedded CIDFontType2 CIDToGIDMap uses Identity (9.7.4.1, Table 115).
pub const CID_TO_GID_IDENTITY: Repair = Repair::new("font.cid-to-gid-identity");
/// Faulty extraction-only map is discarded (9.10.3).
pub const TOUNICODE_INVALID: Repair = Repair::new("font.tounicode-invalid");
/// Simple-font two-byte ToUnicode sources use their low byte (9.10.3).
pub const TOUNICODE_SIMPLE_CODES: Repair = Repair::new("font.tounicode-simple-codes");
/// Invalid UTF-16BE destinations are omitted (9.10.3).
pub const TOUNICODE_UTF16: Repair = Repair::new("font.tounicode-utf16");
/// Overflowing bfrange destinations increment with byte carry (9.10.3).
pub const TOUNICODE_RANGE: Repair = Repair::new("font.tounicode-range");
/// CMap blocks above 100 entries are accepted within budgets (9.7.5.4; TN5014 7.4).
pub const CMAP_BLOCK_COUNT: Repair = Repair::new("font.cmap-block-count");
/// Malformed byte rectangles use disjoint integer-interval pieces (9.7.6.2).
pub const CMAP_CODESPACE: Repair = Repair::new("font.cmap-codespace");
/// Invalid optional FontWeight is ignored when selecting a substitute
/// (9.8.1, Table 120); normal weight and the independent ForceBold flag apply.
pub const DESCRIPTOR_WEIGHT: Repair = Repair::new("font.descriptor-weight");
/// A font descriptor whose `/Flags` is missing or sets both or neither of
/// Symbolic and Nonsymbolic (9.8.2, Table 121). Symbolic stays the primary
/// bit; source objects are not rewritten.
pub const DESCRIPTOR_FLAGS: Repair = Repair::new("font.descriptor-flags");
/// Reversed d1 bounds are ignored only for procedures with no drawing commands
/// (9.6.4, Table 111); the declared horizontal advance is retained.
pub const TYPE3_EMPTY_BOUNDS: Repair = Repair::new("font.type3-empty-bounds");
/// A simple font's `/Encoding` or `/BaseEncoding` names no predefined encoding
/// (Table 112): the font's default encoding applies.
pub const ENCODING_NAME: Repair = Repair::new("font.encoding-name");
/// A simple font's `Widths` array whose length disagrees with `FirstChar` and
/// `LastChar` (Table 109): codes past its end use `MissingWidth` and extra
/// entries are ignored.
pub const WIDTHS_LENGTH: Repair = Repair::new("font.widths-length");
/// A font stream whose `Length1` to `Length3` are absent, invalid or disagree
/// with its decoded program (Table 125). The program is parsed as decoded.
pub const STREAM_LENGTHS: Repair = Repair::new("font.stream-lengths");

/// An embedded font program that cannot be used, even with the parser's own
/// repairs, is replaced by the substitute a non-embedded font with the same
/// name and descriptor would get (ADR 0009); the PDF's widths keep the advances.
pub const EMBEDDED_UNUSABLE: Repair = Repair::new("font.embedded-unusable");

/// Every repair tag defined by this crate.
pub const ALL: &[Repair] = &[
    CID_TO_GID_IDENTITY,
    TOUNICODE_INVALID,
    TOUNICODE_SIMPLE_CODES,
    TOUNICODE_UTF16,
    TOUNICODE_RANGE,
    CMAP_BLOCK_COUNT,
    CMAP_CODESPACE,
    DESCRIPTOR_FLAGS,
    DESCRIPTOR_WEIGHT,
    TYPE3_EMPTY_BOUNDS,
    ENCODING_NAME,
    WIDTHS_LENGTH,
    STREAM_LENGTHS,
    EMBEDDED_UNUSABLE,
];
