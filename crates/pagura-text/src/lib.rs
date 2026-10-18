#![forbid(unsafe_code)]
//! PDF fonts and text decoding (ISO 32000-2 clause 9; ADR 0009).
//!
//! [`PdfFont::load`] interprets a font dictionary — simple fonts (Type 1,
//! MMType1, TrueType, Type 3) and composite fonts (Type 0 with CIDFontType0
//! or CIDFontType2 descendants) — with its encoding or CMap, widths, embedded
//! font program and `/ToUnicode` map. [`PdfFont::decode`] splits a string
//! operand into glyphs carrying everything the renderer and text extraction
//! need. Positioning glyphs on the page (text state, `Tm`, `TJ` adjustments)
//! belongs to the content interpreter in `pagura-render`.
//!
//! [`PdfFont::load_with`] accepts a [`Provider`] for non-embedded programs and
//! predefined CMaps; [`PdfFont::load`] retains explicit failures without one.
//! Font-program parsing is delegated to `pagura-font`. TrueType selection uses the program's subtable availability
//! and post names (9.6.5.4). Glyph-name Unicode fallback implements AGL name
//! decomposition with Latin, Dingbats and common Symbol names; unbundled legacy
//! names remain unknown. A `/ToUnicode` sequence always takes precedence.
//!
//! Loading and decoding enforce document limits. CID intervals stay compact;
//! Unicode expansion, inherited CMap graphs and decoded glyph counts are bounded.

mod cid;
mod cmap;
mod descriptor;
mod encoding;
pub mod features;
mod font;
mod glyph;
pub mod information;
mod memory;
mod metrics;
mod objects;
mod program;
mod provider;
pub mod repairs;
mod report;
mod strings;
mod substitute;
pub use strings::decode_text_string;

pub use provider::{
    CMapCode, CidToUnicode, PredefinedCMap, Provider, StandardDescriptor, StandardFont,
    StandardMetrics,
};
mod selection;
mod tables;

/// Converts an Adobe glyph name to Unicode using the same AGL name rules
/// as PDF encoding lookup (ISO 32000-2 9.10.2). Unknown names return `None`.
/// Suffixes, underscore components and `uni`/`u` names follow the existing
/// encoding conversion. ZapfDingbats-only names are excluded. Work and output
/// grow linearly with the name length;
/// callers account for their input and output budgets.
/// This function neither selects a glyph nor consults ToUnicode.
pub fn glyph_name_unicode(name: &[u8]) -> Option<String> {
    encoding::unicode(name)
}

/// Converts a glyph name using AGL, with the separate ZapfDingbats list only
/// for a recognized ZapfDingbats/Dingbats base font (9.10.2, Annex D.6).
/// Recognized subset and style names use the same family rules as substitution.
/// Mapping priority, glyph selection and budgets remain the caller's responsibility.
pub fn glyph_name_unicode_for_font(name: &[u8], base_font: &[u8]) -> Option<String> {
    encoding::unicode_for_font(name, base_font)
}

pub use font::{FontKind, PdfFont, WritingMode};
pub use glyph::{Glyph, GlyphShape, VerticalMetrics};

#[cfg(test)]
mod substitution_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tolerance_tests;
