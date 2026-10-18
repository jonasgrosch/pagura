#![forbid(unsafe_code)]
//! Font programs: glyph outlines and metrics from TrueType and OpenType
//! (`glyf` and CFF/CFF2 outlines), bare CFF including CID-keyed CFF, and
//! Type 1 (ADR 0009).
//!
//! This crate is PDF-agnostic: it parses font program bytes (the decoded
//! `/FontFile*` streams) and answers glyph questions. Mapping character codes
//! to glyphs through PDF encodings and CMaps is `pagura-text`'s job.
//! Hinting is not applied; outlines are returned in glyph space.

use pagura_core::{Diagnostics, Limits, Name, Recovery, Result};
use pagura_raster::{Path, Transform};
use std::sync::Arc;

mod cff;
mod memory;
mod names;
mod read;
mod recovery;
mod sfnt;
pub mod tags;
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
    /// (3, 10): Windows full Unicode; distinct from PDF's (3, 1) selection.
    WindowsUnicodeFull,
    /// Platform 0 Unicode subtables, distinct from Windows encodings.
    Unicode,
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
    retained_bytes: usize,
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
        Self::parse_with_diagnostics(
            data,
            format,
            limits,
            &mut Diagnostics::new(Recovery::Strict),
        )
        .map_err(recovery::legacy_error)
    }

    /// Parses a font with explicit, reported recovery (ISO 32000-2 9.9).
    ///
    /// Forward the diagnostics to the owning document after this call, including
    /// on error. Original program bytes are retained unchanged. Diagnostics are
    /// coalesced by repair kind per call; the first buffer position is retained.
    ///
    /// # Errors
    ///
    /// Strict mode rejects each repair with its stable tag. Required tables,
    /// undecodable containers and exhausted resource budgets remain errors.
    pub fn parse_with_diagnostics(
        data: impl Into<Arc<[u8]>>,
        format: FontFormat,
        limits: &Limits,
        diagnostics: &mut Diagnostics,
    ) -> Result<Self> {
        let mut local = Diagnostics::new(diagnostics.mode());
        let result = Self::parse_inner(data.into(), format, limits, &mut local);
        diagnostics.absorb(local.take())?;
        result
    }

    fn parse_inner(
        data: Arc<[u8]>,
        format: FontFormat,
        limits: &Limits,
        diagnostics: &mut Diagnostics,
    ) -> Result<Self> {
        read::input(&data, limits)?;
        let parsed = match format {
            FontFormat::TrueType | FontFormat::OpenType => {
                Program::Sfnt(sfnt::Sfnt::parse(&data, limits, diagnostics)?)
            }
            FontFormat::Cff => {
                Program::Cff(cff::Cff::parse(&data, 0..data.len(), limits, diagnostics)?)
            }
            FontFormat::Type1 => Program::Type1(type1::Type1::parse(&data, limits, diagnostics)?),
        };
        let retained_bytes = memory::estimate(&data, &parsed);
        Ok(Self {
            format,
            data,
            parsed: Arc::new(parsed),
            limits: *limits,
            retained_bytes,
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

    /// Estimated heap bytes retained by this parsed program (ADR 0012).
    ///
    /// Computed once at parse time with saturating arithmetic. Counts the input
    /// and parsed-state Arc allocations, owned vector capacities, decrypted
    /// Type 1 buffers, encodings, name/CID indexes and variation data. Tree
    /// entries use 64 bytes of estimated bookkeeping; name payloads are charged
    /// by length, conservatively including names that core may store inline.
    ///
    /// This is an accounting estimate, not allocator telemetry or peak memory.
    /// It excludes this value's inline storage, any caller's Arc around it,
    /// temporary parse/outline storage and caller-owned diagnostics. Clones
    /// share the counted allocations: charge the estimate once per distinct
    /// program, rather than once per clone. Outlines are not retained.
    pub fn retained_bytes(&self) -> usize {
        self.retained_bytes
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
        self.outline_with_diagnostics(glyph, &mut Diagnostics::new(Recovery::Strict))
            .map_err(recovery::legacy_error)
    }

    /// Outlines one glyph with explicit recovery and diagnostics.
    ///
    /// In repair mode an unusable glyph returns an empty path with
    /// [`tags::GLYPH_DATA`]; an unknown charstring operator ends only that glyph.
    /// Other glyphs and PDF advances are unaffected. Forward diagnostics to
    /// `Document::report` even on error; this crate has no document dependency.
    ///
    /// # Errors
    ///
    /// Strict mode returns tagged repair rejections. Exhausted budgets,
    /// overflowing sizes and nonfinite geometry remain errors
    /// in both modes. A caller must never suppress those failures.
    pub fn outline_with_diagnostics(
        &self,
        glyph: GlyphId,
        diagnostics: &mut Diagnostics,
    ) -> Result<Path> {
        let mut local = Diagnostics::new(diagnostics.mode());
        let result = self.outline_inner(glyph, &mut local);
        let result = match result {
            Err(error) if recovery::glyph_error(&error) => {
                recovery::repair(&mut local, tags::GLYPH_DATA, error.message, error.position)
                    .map(|()| Path::default())
            }
            other => other,
        };
        diagnostics.absorb(local.take())?;
        result
    }

    fn outline_inner(&self, glyph: GlyphId, diagnostics: &mut Diagnostics) -> Result<Path> {
        let path = match self.parsed.as_ref() {
            Program::Sfnt(p) => p.outline(&self.data, glyph, &self.limits, diagnostics),
            Program::Cff(p) => p.outline(&self.data, glyph, &self.limits, diagnostics),
            Program::Type1(p) => p.outline(glyph, &self.limits, diagnostics),
        }?;
        read::finite_path(&path)?;
        Ok(path)
    }

    /// The horizontal advance of `glyph` in glyph space, if the program
    /// records one. Malformed charstrings return `None`; call [`Self::outline`]
    /// to obtain their structured error.
    pub fn advance(&self, glyph: GlyphId) -> Option<f64> {
        match self.parsed.as_ref() {
            Program::Sfnt(p) => p.advance(&self.data, glyph, &self.limits),
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

    /// The program's name for a glyph: TrueType `post`, or a name-keyed
    /// Type 1/CFF charset. Unnamed glyphs and bare CID-keyed CFF/CFF2 charsets
    /// return `None`; an OpenType container can also provide `post` names.
    /// This constant-time metadata lookup does not select or change a glyph.
    /// Readers may use it after other Unicode mappings fail (PDF 9.10.2).
    pub fn glyph_name(&self, glyph: GlyphId) -> Option<&Name> {
        match self.parsed.as_ref() {
            Program::Sfnt(p) => p.glyph_name(glyph),
            Program::Cff(p) => p.glyph_name(glyph),
            Program::Type1(p) => p.glyph_name(glyph),
        }
    }

    /// Whether the program has a usable TrueType `cmap` subtable of this kind
    /// (formats 0, 2, 4, 6, 10, 12 or 13). How a PDF character code selects a TrueType
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
