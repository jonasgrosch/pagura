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
