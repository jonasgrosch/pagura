//! Stable repairs for font programs (ISO 32000-2 9.9; ADR 0003).
use pagura_core::Repair;

/// Infer the sfnt flavor from intact required tables when its version is unknown.
pub const SFNT_VERSION: Repair = Repair::new("font.sfnt-version");
/// Read an sfnt table at its declared byte offset despite missing alignment.
pub const TABLE_ALIGNMENT: Repair = Repair::new("font.table-alignment");
/// Clamp an sfnt table to available bytes; required metadata must remain usable.
pub const TABLE_BOUNDS: Repair = Repair::new("font.table-bounds");
/// Retain complete cmap entries and omit unavailable mappings.
pub const CMAP_TRUNCATED: Repair = Repair::new("font.cmap-truncated");
/// Omit an unusable cmap subtable while retaining other mappings.
pub const CMAP_DATA: Repair = Repair::new("font.cmap-data");
/// Read byte-aligned cmap glyph words and ignore a broken sentinel offset.
pub const CMAP_ALIGNMENT: Repair = Repair::new("font.cmap-alignment");
/// Ignore reserved composite flags, without accepting conflicting transforms.
pub const COMPONENT_FLAGS: Repair = Repair::new("font.component-flags");
/// End a glyph at an unknown charstring operator, retaining its completed prefix.
pub const CHARSTRING_OPERATOR: Repair = Repair::new("font.charstring-operator");
/// Replace an unusable glyph with an empty outline; resource failures stay errors.
pub const GLYPH_DATA: Repair = Repair::new("font.glyph-data");

/// Ignore blue hint ordering faults in unhinted outlines (TN 5176 section 15;
/// Type 1 5.3-5.5). Operator prerequisites and ascending pairs still apply in
/// strict mode. Original bytes are retained; no hint zones are sorted.
pub const CFF_BLUE_ORDER: Repair = Repair::new("font.cff-blue-order");

/// Ignore reserved CFF DICT operator slots and their operands (TN 5176 section 4,
/// Appendix H). Known operators in the wrong dictionary remain errors.
pub const CFF_DICT_OPERATOR: Repair = Repair::new("font.cff-dict-operator");
/// Omit unused Private DICT hint values with invalid arity (TN 5176 section 15,
/// Table 23). Widths, subroutine offsets and random seeds remain errors.
pub const CFF_DICT_OPERANDS: Repair = Repair::new("font.cff-dict-operands");
/// Retain the first Type 1 charstring for a repeated name (Type 1 chapter 6).
/// This is reported policy, not PostScript dictionary replacement semantics.
pub const TYPE1_DUPLICATE_GLYPH: Repair = Repair::new("font.type1-duplicate-glyph");
/// Drop unusable supplemental mappings, preserving the main encoding
/// (TN 5176 section 12, Tables 14/15).
pub const CFF_ENCODING_SUPPLEMENT: Repair = Repair::new("font.cff-encoding-supplement");
