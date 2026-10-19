//! Stable tags for the reported repairs of damaged codec data (ADR 0003).
use pagura_core::Repair;

/// Ignore non-one unused entropy bits at a scan or restart boundary
/// (T.81 B.1.1.5 and E.1.4); entropy symbols are never synthesized.
pub const JPEG_ENTROPY_PADDING: Repair = Repair::new("jpeg.entropy-padding");
/// Use the known 12-byte Adobe APP14 prefix and ignore an extension
/// (7.4.8, Table 13). The extended layout is reader policy; Adobe Technical
/// Note 5116 remains pending primary-text review.
pub const JPEG_ADOBE_MARKER: Repair = Repair::new("jpeg.adobe-marker");
/// Ignore excess entropy bytes after the prescribed MCUs only when the next
/// unstuffed marker is EOI (T.81 B.2.1). Never skip another scan.
pub const JPEG_ENTROPY_EXCESS: Repair = Repair::new("jpeg.entropy-excess");
/// Discard a restart marker immediately before EOI after all required MCUs
/// were decoded (T.81 B.2.1, B.2.4.4); no entropy is synthesized.
pub const JPEG_TRAILING_RESTART: Repair = Repair::new("jpeg.trailing-restart");
/// Substitute the previous good row, or white after a damaged row, after
/// locating the next EOL (7.4.6, Table 11).
pub const CCITT_DAMAGED_ROW: Repair = Repair::new("ccitt.damaged-row");
/// Accept up to seven zero fill bits before a byte-aligned Group 4 EOFB.
/// T.6 2.4.1.1-2 and Table 11 permit padding after the EOFB instead.
pub const CCITT_EOB_PADDING: Repair = Repair::new("ccitt.eob-padding");
/// Ordered tile-parts exceeding their declared count use their actual,
/// length-delimited sequence through EOC (T.800 A.4.2).
pub const JPX_TILE_PART_COUNT: Repair = Repair::new("jpx.tile-part-count");
/// A sole top-level component mapping box before the codestream supplies the
/// header palette's missing mapping (T.800 I.5.3.4, I.5.3.5).
pub const JPX_MAPPING_LOCATION: Repair = Repair::new("jpx.mapping-box-location");

/// Every repair tag of this crate.
pub const ALL: &[Repair] = &[
    JPEG_ENTROPY_PADDING,
    JPEG_ADOBE_MARKER,
    JPEG_ENTROPY_EXCESS,
    JPEG_TRAILING_RESTART,
    CCITT_DAMAGED_ROW,
    CCITT_EOB_PADDING,
    JPX_TILE_PART_COUNT,
    JPX_MAPPING_LOCATION,
];
