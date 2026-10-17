#![forbid(unsafe_code)]
//! Font programs: glyph outlines and metrics from TrueType and OpenType
//! (`glyf` and CFF/CFF2 outlines), bare CFF including CID-keyed CFF, and
//! Type 1 (ADR 0009).
//!
//! This crate is PDF-agnostic: it parses font program bytes (the decoded
//! `/FontFile*` streams) and answers glyph questions. Mapping character codes
//! to glyphs through PDF encodings and CMaps is `pagura-text`'s job.
//! Hinting is not applied; outlines are returned in glyph space.

use pagura_core::{Error, Feature, Limits, Name, Result};
use pagura_raster::{Path, Transform};
use std::sync::Arc;

/// Stable tags for font capabilities not implemented yet.
pub mod features {
    use pagura_core::Feature;

    /// A font program format this crate cannot read.
    pub const FORMAT: Feature = Feature::new("font.format");
}

/// Placeholder tag while the parsers are under construction. Must not
/// survive the implementation.
const PENDING: Feature = Feature::new("font.pending");

/// The container format of a font program, as named by the PDF stream that
/// holds it (Table 126: `/FontFile`, `/FontFile2`, `/FontFile3` subtypes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FontFormat {
    /// Type 1 (`/FontFile`): cleartext and eexec-encrypted portions.
    Type1,
    /// TrueType (`/FontFile2`) with `glyf` outlines.
    TrueType,
    /// Bare CFF (`/FontFile3 /Subtype /Type1C` or `/CIDFontType0C`).
    Cff,
    /// OpenType (`/FontFile3 /Subtype /OpenType`): `glyf` or CFF/CFF2 outlines.
    OpenType,
}

/// A glyph index within one font program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GlyphId(pub u32);

/// A TrueType `cmap` subtable selection (platform, encoding).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CmapSubtable {
    /// (3, 1): Windows Unicode BMP.
    WindowsUnicode,
    /// (3, 0): Windows Symbol.
    WindowsSymbol,
    /// (1, 0): Macintosh Roman.
    MacRoman,
}

/// A parsed font program. Cheap to clone; the bytes are shared.
#[derive(Debug, Clone)]
pub struct FontProgram {
    format: FontFormat,
    data: Arc<[u8]>,
}

impl FontProgram {
    /// Parses a font program, validating its tables within `limits`.
    ///
    /// # Errors
    ///
    /// Returns a syntax error for malformed programs, `Unsupported` for
    /// formats or features not implemented, and limit errors for budgets.
    pub fn parse(data: impl Into<Arc<[u8]>>, format: FontFormat, limits: &Limits) -> Result<Self> {
        let _: Arc<[u8]> = data.into();
        let _ = (format, limits);
        Err(Error::unsupported(
            PENDING,
            "font programs are not implemented yet",
        ))
    }

    /// The container format.
    pub fn format(&self) -> FontFormat {
        self.format
    }

    /// The raw program bytes.
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Number of glyphs.
    pub fn glyph_count(&self) -> u32 {
        0
    }

    /// Whether glyphs are selected by CID (CID-keyed CFF).
    pub fn is_cid_keyed(&self) -> bool {
        false
    }

    /// Glyph space to text space: `/FontMatrix` for Type 1 and CFF, and
    /// `1 / unitsPerEm` scaling for TrueType outlines (clause 9.2.4).
    pub fn font_matrix(&self) -> Transform {
        Transform::scale(0.001, 0.001)
    }

    /// The outline of `glyph` in glyph space. An empty path is a valid,
    /// blank glyph.
    ///
    /// # Errors
    ///
    /// Returns a syntax error for malformed glyph data (including charstring
    /// stack or subroutine depth violations) and limit errors for budgets.
    pub fn outline(&self, glyph: GlyphId) -> Result<Path> {
        let _ = glyph;
        Err(Error::unsupported(
            PENDING,
            "glyph outlines are not implemented yet",
        ))
    }

    /// The horizontal advance of `glyph` in glyph space, if the program
    /// records one.
    pub fn advance(&self, glyph: GlyphId) -> Option<f64> {
        let _ = glyph;
        None
    }

    /// The glyph named `name` (Type 1 and CFF charsets, TrueType `post`).
    pub fn glyph_for_name(&self, name: &[u8]) -> Option<GlyphId> {
        let _ = name;
        None
    }

    /// The glyph for `code` in a TrueType `cmap` subtable.
    pub fn glyph_for_code(&self, subtable: CmapSubtable, code: u32) -> Option<GlyphId> {
        let _ = (subtable, code);
        None
    }

    /// The glyph for `cid` in a CID-keyed CFF program.
    pub fn glyph_for_cid(&self, cid: u32) -> Option<GlyphId> {
        let _ = cid;
        None
    }

    /// The program's built-in encoding (Type 1 and CFF), code to glyph name.
    pub fn builtin_encoding(&self) -> Option<&[Option<Name>; 256]> {
        None
    }
}
