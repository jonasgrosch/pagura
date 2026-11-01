#![forbid(unsafe_code)]
#![warn(unreachable_pub)]
//! Font programs: glyph outlines and metrics from TrueType and OpenType
//! (`glyf` and CFF/CFF2 outlines), bare CFF including CID-keyed CFF, and
//! Type 1 (ADR 0009, ADR 0023).
//!
//! This crate is PDF-agnostic: it parses font program bytes (the decoded
//! `/FontFile*` streams) and answers glyph questions. Mapping character codes
//! to glyphs through PDF encodings and CMaps is `pagura-text`'s job.
//! Hinting is not applied; outlines are returned in glyph space.

use pagura_core::{Diagnostic, Diagnostics, Error, Limits, Name, Position, Recovery, Result};
use pagura_raster::{Path, Transform};
use std::ops::Deref;
use std::sync::Arc;

mod budget;
mod cff;
mod charstring;
mod memory;
mod names;
mod outline;
mod read;
mod recovery;
pub mod repairs;
mod sfnt;
mod type1;

use budget::{FontBudget, Work};

/// Stable tags for font capabilities not implemented yet.
pub mod features {
    use pagura_core::Feature;

    /// A font program format this crate cannot read.
    pub const FORMAT: Feature = Feature::new("font.format");
    /// Type 1 procedures requiring a PostScript execution environment.
    pub const TYPE1_POSTSCRIPT: Feature = Feature::new("font.type1-postscript");
    /// CFF2 variation expressions outside the default-instance interpreter.
    pub const VARIATIONS: Feature = Feature::new("font.variations");

    /// Every feature tag of this crate.
    pub const ALL: &[Feature] = &[FORMAT, TYPE1_POSTSCRIPT, VARIATIONS];
}

/// The container format of a font program, as named by the PDF stream that
/// holds it (9.9, Table 124: `/FontFile`, `/FontFile2`, `/FontFile3` subtypes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
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

/// Selects a face in an OpenType or TrueType collection (OpenType TTC Header).
/// Standalone programs have index zero and ignore name hints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FontFace<'a> {
    /// Zero-based collection index. Ordinary parsing selects index zero.
    Index(u32),
    /// Ordered PostScript name hints. The first hint matching any face must
    /// identify exactly one; an ambiguous match does not fall back to a later
    /// hint. Missing or ambiguous matches are errors. Names are compared
    /// exactly with naming-table ID 6, independently of the CFF Name INDEX.
    PostScriptNames(&'a [&'a [u8]]),
}

/// A glyph index within one font program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GlyphId(pub u32);

/// A TrueType `cmap` subtable selection (platform, encoding).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
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

/// The bytes of a font program, shared by every clone of the parsed
/// program.
///
/// Data that lives for the whole process, such as fonts bundled with
/// `include_bytes!`, is used in place rather than copied. Convert with
/// `From`: `&'static [u8]` becomes [`FontBytes::Static`], and `Vec<u8>`,
/// `Box<[u8]>` and `Arc<[u8]>` become [`FontBytes::Shared`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum FontBytes {
    /// Bytes that outlive every program, used without copying.
    Static(&'static [u8]),
    /// Heap bytes, shared by reference counting.
    Shared(Arc<[u8]>),
}

impl FontBytes {
    /// The bytes.
    pub fn as_slice(&self) -> &[u8] {
        match self {
            Self::Static(bytes) => bytes,
            Self::Shared(bytes) => bytes,
        }
    }

    /// Heap bytes held for the program: none for static data.
    fn heap_len(&self) -> usize {
        match self {
            Self::Static(_) => 0,
            Self::Shared(bytes) => bytes.len(),
        }
    }
}

impl Deref for FontBytes {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl AsRef<[u8]> for FontBytes {
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl From<&'static [u8]> for FontBytes {
    fn from(bytes: &'static [u8]) -> Self {
        Self::Static(bytes)
    }
}

impl<const N: usize> From<&'static [u8; N]> for FontBytes {
    fn from(bytes: &'static [u8; N]) -> Self {
        Self::Static(bytes)
    }
}

impl From<Arc<[u8]>> for FontBytes {
    fn from(bytes: Arc<[u8]>) -> Self {
        Self::Shared(bytes)
    }
}

impl From<Vec<u8>> for FontBytes {
    fn from(bytes: Vec<u8>) -> Self {
        Self::Shared(bytes.into())
    }
}

impl From<Box<[u8]>> for FontBytes {
    fn from(bytes: Box<[u8]>) -> Self {
        Self::Shared(bytes.into())
    }
}

/// A parsed font program. Cheap to clone; the bytes are shared.
#[derive(Debug, Clone)]
pub struct FontProgram {
    format: FontFormat,
    data: FontBytes,
    parsed: Arc<Program>,
    budget: FontBudget,
    retained_bytes: usize,
}

#[derive(Debug)]
enum Program {
    Sfnt(sfnt::Sfnt),
    Cff(cff::Cff),
    Type1(type1::Type1),
}

impl FontProgram {
    /// Parses a font program strictly, validating its tables within
    /// `limits`.
    ///
    /// # Errors
    ///
    /// Returns a positioned `Syntax` error for a malformed program, `Limit`
    /// when a font budget of `limits` is exhausted (`font_work`,
    /// `font_entries`, `decoded_bytes`, `string_bytes`, `depth`), and
    /// `Unsupported` for unimplemented formats or features. A repair the
    /// program would need is rejected with its tag. Glyph programs are
    /// validated on demand by [`Self::outline`].
    pub fn parse(data: impl Into<FontBytes>, format: FontFormat, limits: &Limits) -> Result<Self> {
        Self::parse_with_diagnostics(
            data,
            format,
            limits,
            &mut Diagnostics::new(Recovery::Strict),
        )
    }

    /// Parses a font with explicit, reported recovery (9.9).
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
        data: impl Into<FontBytes>,
        format: FontFormat,
        limits: &Limits,
        diagnostics: &mut Diagnostics,
    ) -> Result<Self> {
        Self::parse_with_face(data, format, FontFace::Index(0), limits, diagnostics)
    }

    /// Parses a selected collection face with explicit, reported recovery.
    ///
    /// The entire immutable input is retained once. Only the selected face's
    /// font tables are parsed; name selection scans bounded directory and name
    /// records without decoding the other faces' glyph programs. Standalone
    /// programs ignore name hints, preserving their ordinary parsing behavior.
    ///
    /// # Errors
    ///
    /// As [`Self::parse_with_diagnostics`]. An out-of-range index is an
    /// `InvalidArgument`; absent or ambiguous collection names are `Syntax`.
    pub fn parse_with_face(
        data: impl Into<FontBytes>,
        format: FontFormat,
        face: FontFace<'_>,
        limits: &Limits,
        diagnostics: &mut Diagnostics,
    ) -> Result<Self> {
        let mut local = Diagnostics::new(diagnostics.mode());
        let result = Self::parse_inner(
            data.into(),
            format,
            face,
            FontBudget::new(limits),
            &mut local,
        );
        diagnostics.absorb(local.take())?;
        result
    }

    fn parse_inner(
        data: FontBytes,
        format: FontFormat,
        face: FontFace<'_>,
        budget: FontBudget,
        diagnostics: &mut Diagnostics,
    ) -> Result<Self> {
        budget.input(data.len())?;
        let mut work = Work::new(budget, diagnostics);
        let parsed = match format {
            FontFormat::TrueType | FontFormat::OpenType => {
                Program::Sfnt(sfnt::Sfnt::parse(&data, face, &mut work)?)
            }
            FontFormat::Cff | FontFormat::Type1 => {
                if matches!(face, FontFace::Index(index) if index != 0) {
                    return Err(Error::invalid_argument(
                        "standalone font index must be zero",
                    ));
                }
                match format {
                    FontFormat::Cff => {
                        Program::Cff(cff::Cff::parse(&data, 0..data.len(), &mut work)?)
                    }
                    _ => Program::Type1(type1::Type1::parse(&data, &mut work)?),
                }
            }
        };
        let retained_bytes = memory::estimate(&data, &parsed);
        Ok(Self {
            format,
            data,
            parsed: Arc::new(parsed),
            budget,
            retained_bytes,
        })
    }

    /// The container format.
    pub const fn format(&self) -> FontFormat {
        self.format
    }

    /// The raw program bytes.
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Estimated heap bytes retained by this parsed program (ADR 0012).
    ///
    /// Computed once at parse time with saturating arithmetic. Counts shared
    /// input bytes (not [`FontBytes::Static`] data, which is not heap), the
    /// Arc allocations, owned vector capacities, decrypted Type 1 buffers,
    /// encodings, name/CID indexes and variation data. Tree entries use
    /// `pagura_core::INDEX_ENTRY_OVERHEAD` bytes of estimated bookkeeping;
    /// name payloads are charged by length, conservatively including names
    /// that core may store inline.
    ///
    /// This is an accounting estimate, not allocator telemetry or peak memory.
    /// It excludes this value's inline storage, any caller's Arc around it,
    /// temporary parse/outline storage and caller-owned diagnostics. Clones
    /// share the counted allocations: charge the estimate once per distinct
    /// program, rather than once per clone. Outlines are not retained.
    pub const fn retained_bytes(&self) -> usize {
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

    /// Whether the selected program uses CFF/CFF2 charstrings, including
    /// OpenType collection faces. This inspects the parsed outline family,
    /// independently of the caller's container-format hint.
    pub fn has_cff_outlines(&self) -> bool {
        match self.parsed.as_ref() {
            Program::Sfnt(program) => program.has_cff_outlines(),
            Program::Cff(_) => true,
            Program::Type1(_) => false,
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
    /// `1 / unitsPerEm` scaling for TrueType outlines (9.2.4).
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
    /// Returns a positioned `Syntax` error for malformed glyph data,
    /// including format ceilings such as stack and subroutine-depth
    /// violations, `Limit` when the outline exhausts a budget of the parse
    /// limits (`font_work`, `font_entries`, `path_segments`, `depth`), and a
    /// rejected repair with its tag.
    pub fn outline(&self, glyph: GlyphId) -> Result<Path> {
        self.outline_with_diagnostics(glyph, &mut Diagnostics::new(Recovery::Strict))
    }

    /// Outlines one glyph with explicit recovery and diagnostics.
    ///
    /// In repair mode an unusable (malformed or unsupported) glyph returns
    /// an empty path with [`repairs::GLYPH_DATA`]; an unknown charstring
    /// operator ends only that glyph. Other glyphs and PDF advances are
    /// unaffected. Forward diagnostics to `Document::report` even on error;
    /// this crate has no document dependency.
    ///
    /// # Errors
    ///
    /// Strict mode returns tagged repair rejections. Exhausted budgets and
    /// overflowing sizes remain `Limit` errors in both modes. A caller must
    /// never suppress those failures.
    pub fn outline_with_diagnostics(
        &self,
        glyph: GlyphId,
        diagnostics: &mut Diagnostics,
    ) -> Result<Path> {
        let mut local = Diagnostics::new(diagnostics.mode());
        let result = match self.outline_inner(glyph, &mut local) {
            Err(error) if recovery::glyph_error(&error) => local
                .repair_once(Diagnostic::new(repairs::GLYPH_DATA, error.message).at(error.position))
                .map(|()| Path::default()),
            other => other,
        };
        diagnostics.absorb(local.take())?;
        result
    }

    fn outline_inner(&self, glyph: GlyphId, diagnostics: &mut Diagnostics) -> Result<Path> {
        let mut work = Work::new(self.budget, diagnostics);
        match self.parsed.as_ref() {
            Program::Sfnt(p) => p.outline(&self.data, glyph, &mut work),
            Program::Cff(p) => p.outline(&self.data, glyph, &mut work),
            Program::Type1(p) => p.outline(glyph, &mut work),
        }
    }

    /// The horizontal advance of `glyph` in glyph space: the `hmtx` advance
    /// when the program has one, otherwise the charstring width. `None` if
    /// the program records no advance (CFF2 without `hmtx`).
    ///
    /// A charstring is interpreted only up to its width, so a fault later
    /// in the glyph is reported by [`Self::outline`] alone.
    ///
    /// # Errors
    ///
    /// A `Syntax` error for a glyph outside the program or a malformed
    /// width, and `Limit` for an exhausted budget, as for [`Self::outline`].
    pub fn advance(&self, glyph: GlyphId) -> Result<Option<f64>> {
        if glyph.0 >= self.glyph_count() {
            return Err(Error::syntax("glyph index outside font program").at(Position::Unknown));
        }
        let mut diagnostics = Diagnostics::new(Recovery::Strict);
        let mut work = Work::new(self.budget, &mut diagnostics);
        match self.parsed.as_ref() {
            Program::Sfnt(p) => p.advance(&self.data, glyph, &mut work),
            Program::Cff(p) => p.advance(&self.data, glyph, &mut work),
            Program::Type1(p) => p.advance(glyph, &mut work).map(Some),
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
    /// Type 1/CFF charset. Unnamed glyphs and bare CID-keyed CFF/CFF2
    /// charsets return `None`; an OpenType container can also provide
    /// `post` names. This constant-time metadata lookup does not select or
    /// change a glyph. Readers may use it after other Unicode mappings fail
    /// (9.10.2).
    pub fn glyph_name(&self, glyph: GlyphId) -> Option<&Name> {
        match self.parsed.as_ref() {
            Program::Sfnt(p) => p.glyph_name(glyph),
            Program::Cff(p) => p.glyph_name(glyph),
            Program::Type1(p) => p.glyph_name(glyph),
        }
    }

    /// Whether the program has a usable TrueType `cmap` subtable of this kind
    /// (formats 0, 2, 4, 6, 10, 12 or 13). How a PDF character code selects a TrueType
    /// glyph depends on which subtables exist (9.6.5.4).
    pub fn has_cmap_subtable(&self, subtable: CmapSubtable) -> bool {
        match self.parsed.as_ref() {
            Program::Sfnt(p) => p.has_cmap(subtable),
            Program::Cff(_) | Program::Type1(_) => false,
        }
    }

    /// The glyph for `code` in a TrueType `cmap` subtable. Subtables are
    /// validated at parse time; a code that a repaired, truncated subtable
    /// no longer maps has no glyph.
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
