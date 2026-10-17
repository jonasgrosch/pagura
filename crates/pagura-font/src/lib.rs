#![forbid(unsafe_code)]
//! Font programs: glyph outlines and metrics from TrueType and OpenType
//! (`glyf` and CFF/CFF2 outlines), bare CFF including CID-keyed CFF, and
//! Type 1 (ADR 0009).
//!
//! This crate is PDF-agnostic: it parses font program bytes (the decoded
//! `/FontFile*` streams) and answers glyph questions. Mapping character codes
//! to glyphs through PDF encodings and CMaps is `pagura-text`'s job.
//! Hinting is not applied; outlines are returned in glyph space.

use pagura_core::{Limits, Name, Result};
use pagura_raster::{Path, Transform};
use std::sync::Arc;

mod cff;
mod read;
mod sfnt;
mod type1;

#[cfg(test)]
mod tests;

/// Stable tags for font capabilities not implemented yet.
pub mod features {
    use pagura_core::Feature;

    /// A font program format this crate cannot read.
    pub const FORMAT: Feature = Feature::new("font.format");
    /// Type 1 procedures requiring a PostScript execution environment.
    pub const TYPE1_POSTSCRIPT: Feature = Feature::new("font.type1-postscript");
    /// CFF2 variation expressions outside the default-instance interpreter.
    pub const VARIATIONS: Feature = Feature::new("font.variations");
}

/// The container format of a font program, as named by the PDF stream that
/// holds it (ISO 32000-2 clause 9.9, Table 124: `/FontFile`, `/FontFile2`, `/FontFile3` subtypes).
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
    parsed: Arc<Program>,
    limits: Limits,
}

#[derive(Debug)]
enum Program {
    Sfnt(sfnt::Sfnt),
    Cff(cff::Cff),
    Type1(type1::Type1),
}

impl FontProgram {
    /// Parses a font program, validating its tables within `limits`.
    ///
    /// # Errors
    ///
    /// Returns positioned syntax errors for malformed programs or exceeded
    /// font budgets, and `Unsupported` for unimplemented formats or features.
    /// Glyph programs are validated on demand by [`Self::outline`].
    pub fn parse(data: impl Into<Arc<[u8]>>, format: FontFormat, limits: &Limits) -> Result<Self> {
        let data = data.into();
        read::input(&data, limits)?;
        let parsed = match format {
            FontFormat::TrueType | FontFormat::OpenType => {
                Program::Sfnt(sfnt::Sfnt::parse(&data, limits)?)
            }
            FontFormat::Cff => Program::Cff(cff::Cff::parse(&data, 0..data.len(), limits)?),
            FontFormat::Type1 => Program::Type1(type1::Type1::parse(&data, limits)?),
        };
        Ok(Self {
            format,
            data,
            parsed: Arc::new(parsed),
            limits: *limits,
        })
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
        match self.parsed.as_ref() {
            Program::Sfnt(p) => p.glyph_count(),
            Program::Cff(p) => p.glyph_count(),
            Program::Type1(p) => p.glyph_count(),
        }
    }

    /// Whether glyphs are selected by CID (CID-keyed CFF).
    pub fn is_cid_keyed(&self) -> bool {
        match self.parsed.as_ref() {
            Program::Sfnt(p) => p.is_cid(),
            Program::Cff(p) => p.is_cid(),
            Program::Type1(_) => false,
        }
    }

    /// Glyph space to text space: `/FontMatrix` for Type 1 and CFF, and
    /// `1 / unitsPerEm` scaling for TrueType outlines (clause 9.2.4).
    pub fn font_matrix(&self) -> Transform {
        match self.parsed.as_ref() {
            Program::Sfnt(p) => p.matrix(),
            Program::Cff(p) => p.matrix(),
            Program::Type1(p) => p.matrix(),
        }
    }

    /// The outline of `glyph` in glyph space. An empty path is a valid,
    /// blank glyph.
    ///
    /// # Errors
    ///
    /// Returns positioned syntax errors for malformed glyph data or exceeded
    /// font budgets, including stack and subroutine-depth violations.
    pub fn outline(&self, glyph: GlyphId) -> Result<Path> {
        let path = match self.parsed.as_ref() {
            Program::Sfnt(p) => p.outline(&self.data, glyph, &self.limits),
            Program::Cff(p) => p.outline(&self.data, glyph, &self.limits),
            Program::Type1(p) => p.outline(glyph, &self.limits),
        }?;
        read::finite_path(&path)?;
        Ok(path)
    }

    /// The horizontal advance of `glyph` in glyph space, if the program
    /// records one. Malformed charstrings return `None`; call [`Self::outline`]
    /// to obtain their structured error.
    pub fn advance(&self, glyph: GlyphId) -> Option<f64> {
        match self.parsed.as_ref() {
            Program::Sfnt(p) => p.advance(glyph),
            Program::Cff(p) => p.advance(&self.data, glyph, &self.limits),
            Program::Type1(p) => p.advance(glyph, &self.limits),
        }
    }

    /// The glyph named `name` (Type 1 and CFF charsets, TrueType `post`).
    pub fn glyph_for_name(&self, name: &[u8]) -> Option<GlyphId> {
        match self.parsed.as_ref() {
            Program::Sfnt(p) => p.name(name),
            Program::Cff(p) => p.name(name),
            Program::Type1(p) => p.name(name),
        }
    }

    /// Whether the program has a usable TrueType `cmap` subtable of this kind
    /// (formats 0, 4, 6 or 12). How a PDF character code selects a TrueType
    /// glyph depends on which subtables exist (clause 9.6.5.4).
    pub fn has_cmap_subtable(&self, subtable: CmapSubtable) -> bool {
        match self.parsed.as_ref() {
            Program::Sfnt(p) => p.has_cmap(subtable),
            Program::Cff(_) | Program::Type1(_) => false,
        }
    }

    /// The glyph for `code` in a TrueType `cmap` subtable.
    pub fn glyph_for_code(&self, subtable: CmapSubtable, code: u32) -> Option<GlyphId> {
        match self.parsed.as_ref() {
            Program::Sfnt(p) => p.code(&self.data, subtable, code),
            Program::Cff(_) | Program::Type1(_) => None,
        }
    }

    /// The glyph for `cid` in a CID-keyed CFF program.
    pub fn glyph_for_cid(&self, cid: u32) -> Option<GlyphId> {
        match self.parsed.as_ref() {
            Program::Sfnt(p) => p.cid(cid),
            Program::Cff(p) => p.cid(cid),
            Program::Type1(_) => None,
        }
    }

    /// The program's built-in encoding (Type 1 and CFF), code to glyph name.
    pub fn builtin_encoding(&self) -> Option<&[Option<Name>; 256]> {
        match self.parsed.as_ref() {
            Program::Sfnt(p) => p.encoding(),
            Program::Cff(p) => p.encoding(),
            Program::Type1(p) => Some(p.encoding()),
        }
    }
}
