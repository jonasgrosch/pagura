//! Loaded PDF fonts (9.6–9.7): the public font type and string decoding.
use crate::{
    CidToUnicode, Glyph, GlyphText, Provider,
    cid::CidToGidMap,
    cmap::{self, CMap},
    encoding::SimpleMapping,
    features, memory, metrics,
    selection::Route,
};
use pagura_core::{Dictionary, Error, Limits, Meter, Name, ObjectId, Position, Result};
use pagura_document::Document;
use pagura_font::FontProgram;
use pagura_raster::Transform;
use std::{collections::BTreeMap, sync::Arc};

mod descriptor_key;
mod load;
mod shape;
mod unicode;
pub use unicode::FontUnicode;
pub(crate) use unicode::{cid_collection, to_unicode};

/// The font dictionary's `/Subtype`. ISO 32000-2 Table 110 closes this set,
/// so the enum is exhaustive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FontKind {
    /// `Type1`, including the standard 14 fonts.
    Type1,
    /// `MMType1` (multiple master instance).
    MmType1,
    /// `TrueType`.
    TrueType,
    /// `Type3`: glyphs are content streams.
    Type3,
    /// `Type0`: a composite font with one CIDFont descendant.
    Type0,
}

/// Horizontal or vertical writing (clause 9.7.4.3). PDF defines exactly
/// these two modes, so the enum is exhaustive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum WritingMode {
    /// Glyphs advance along x.
    #[default]
    Horizontal,
    /// Glyphs advance along y (CMaps ending in `-V`, `/WMode 1`).
    Vertical,
}

impl FontKind {
    pub(crate) fn from_subtype(name: &[u8]) -> Option<Self> {
        Some(match name {
            b"Type1" => Self::Type1,
            b"MMType1" => Self::MmType1,
            b"TrueType" => Self::TrueType,
            b"Type3" => Self::Type3,
            b"Type0" => Self::Type0,
            _ => return None,
        })
    }

    /// The `/Subtype` name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Type1 => "Type1",
            Self::MmType1 => "MMType1",
            Self::TrueType => "TrueType",
            Self::Type3 => "Type3",
            Self::Type0 => "Type0",
        }
    }
}

impl std::fmt::Display for FontKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where a font's glyphs come from.
#[derive(Debug)]
pub(crate) enum Source {
    MetricsOnly,
    CidSubstitute {
        program: Arc<FontProgram>,
        /// The collection's Unicode, which selects the glyphs.
        unicode: Arc<CidToUnicode>,
    },
    /// A Latin standard face drawing a non-embedded Identity CIDFont with a
    /// Latin family name: each code's ToUnicode text selects the glyph
    /// (ADR 0009). Without a usable ToUnicode map nothing is drawn.
    UnicodeSubstitute {
        program: Arc<FontProgram>,
        to_unicode: Option<Arc<CMap>>,
    },
    Type3 {
        procedures: BTreeMap<Name, ObjectId>,
        resources: Option<Dictionary>,
        matrix: Transform,
    },
    Program {
        program: Arc<FontProgram>,
        route: Route,
        ownership: Ownership,
        cid_map: Option<CidToGidMap>,
    },
}

/// Who owns an outline program, for storage estimates (ADR 0012).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ownership {
    /// Parsed from the document's font stream: the font owns it.
    Embedded,
    /// A provider's substitute: shared, owned by the provider's cache.
    Provider,
}

#[derive(Debug)]
pub(crate) enum Mapping {
    Simple(Box<SimpleMapping>),
    Composite { map: Arc<CMap>, shared: bool },
}

/// Immutable before the first code-selection cache entry is populated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelectionPolicy {
    LegacyNotdef,
    RequiredNamed,
}

/// A loaded PDF font. Load each font dictionary once per document and share it.
#[derive(Debug)]
pub struct PdfFont {
    pub(crate) kind: FontKind,
    pub(crate) writing_mode: WritingMode,
    pub(crate) source: Source,
    pub(crate) mapping: Mapping,
    pub(crate) metrics: metrics::Metrics,
    pub(crate) limits: Limits,
    pub(crate) retained: usize,
    pub(crate) selection_policy: SelectionPolicy,
}

impl PdfFont {
    /// Estimates retained storage, including this font and its owned allocations
    /// (ADR 0012). Computed once at load; this accessor is constant time.
    ///
    /// Counts encoding payloads, width/vertical tables, owned CMaps,
    /// CIDToGIDMap and Type 3 procedures/resources. Tree entries include a
    /// 64-byte bookkeeping allowance; names include a payload allowance even
    /// when inline. Embedded program storage uses `FontProgram::retained_bytes()` plus
    /// the shared handle and Arc bookkeeping.
    /// This is an estimate, not allocator telemetry or a peak-memory bound.
    ///
    /// Provider-owned programs, predefined CMaps and collection Unicode tables
    /// are excluded: their shared allocations belong to the provider's cache
    /// and budget, never to individual fonts. Copies derived from them count.
    /// Document source objects/streams and renderer-owned outline caches are
    /// excluded. Replace an input-size font estimate with this value; do not
    /// add that estimate or `size_of::<PdfFont>()` again.
    pub fn retained_bytes(&self) -> usize {
        self.retained
    }

    pub(crate) fn accounted(mut self) -> Self {
        self.retained = memory::estimate(&self);
        self
    }
    /// Loads the font dictionary `font` (already resolved) of `document`:
    /// what drawing and positioning its glyphs read. The Unicode of its
    /// codes is loaded separately, by [`Self::unicode`].
    ///
    /// # Errors
    ///
    /// Returns an error for malformed dictionaries in Strict mode, `Unsupported`
    /// (for example [`features::NON_EMBEDDED`]) for fonts that cannot be
    /// drawn yet, and errors from loading the font program.
    pub fn load(document: &Document, font: &Dictionary) -> Result<Self> {
        load::load(document, font)
    }

    /// Loads a font with host-supplied substitutes, metrics and predefined
    /// CMaps, when a provider is given. Embedded programs take precedence
    /// (9.6.2.2, 9.7.5.2; ADR 0009). Without a provider this is [`Self::load`].
    ///
    /// # Errors
    /// As [`Self::load`], plus errors from the provider. Unavailable or unsafe
    /// substitutions remain explicit unsupported capabilities.
    pub fn load_with_provider(
        document: &Document,
        font: &Dictionary,
        provider: Option<&dyn Provider>,
    ) -> Result<Self> {
        load::load_with(document, font, provider, Position::Unknown, false)
    }

    /// As [`Self::load_with_provider`], locating the descriptor-flags repair
    /// at `position`, typically the `Tf` operator that selected the font.
    ///
    /// # Errors
    /// As [`Self::load_with_provider`].
    pub fn load_at(
        document: &Document,
        font: &Dictionary,
        provider: Option<&dyn Provider>,
        position: Position,
    ) -> Result<Self> {
        load::load_with(document, font, provider, position, false)
    }

    /// Loads a font for rendering: as [`Self::load_at`], except that an
    /// embedded TrueType program in a `FlateDecode` stream whose `Length1`
    /// covers every table is decoded only up to the end of the last table
    /// that drawing glyphs reads (`head`, `hhea`, `hmtx`, `maxp`, `loca`,
    /// `glyf`, `cmap`, `post`). Tables that end past that point are left out
    /// of the program's table directory, so outlines, metrics and glyph
    /// selection are those of the whole program; the unread tables, the
    /// stream's checksum, its length against `Length1` and any damage after
    /// that point are not examined, and are not reported.
    ///
    /// # Errors
    /// As [`Self::load_with_provider`].
    pub fn load_for_rendering(
        document: &Document,
        font: &Dictionary,
        provider: Option<&dyn Provider>,
        position: Position,
    ) -> Result<Self> {
        load::load_with(document, font, provider, position, true)
    }

    /// Horizontal width in text space, by simple character code or CID.
    /// Available even when a provider supplies metrics without outlines.
    pub fn width(&self, code: u32) -> f64 {
        self.metrics.width(code)
    }

    /// The font's subtype.
    pub fn kind(&self) -> FontKind {
        self.kind
    }

    /// The descriptor key bound by the font's recovery policy, if any.
    ///
    /// This only identifies the descriptor selected while loading the font;
    /// it does not certify coverage of every code subsequently decoded.
    pub fn recovered_descriptor_key(&self) -> Option<&'static [u8]> {
        match self.selection_policy {
            SelectionPolicy::LegacyNotdef => None,
            SelectionPolicy::RequiredNamed => Some(b"GontDescriptor"),
        }
    }

    /// The writing mode.
    pub fn writing_mode(&self) -> WritingMode {
        self.writing_mode
    }

    /// The outline program that draws this font's glyphs: embedded, or a
    /// provider's substitute. `None` for Type 3 fonts and for fonts whose
    /// provider supplies metrics only.
    pub fn program(&self) -> Option<&Arc<FontProgram>> {
        match &self.source {
            Source::Program { program, .. }
            | Source::CidSubstitute { program, .. }
            | Source::UnicodeSubstitute { program, .. } => Some(program),
            Source::Type3 { .. } | Source::MetricsOnly => None,
        }
    }

    /// The `/Resources` of a Type 3 font, for its glyph procedures.
    pub fn type3_resources(&self) -> Option<&Dictionary> {
        match &self.source {
            Source::Type3 { resources, .. } => resources.as_ref(),
            Source::Program { .. }
            | Source::MetricsOnly
            | Source::CidSubstitute { .. }
            | Source::UnicodeSubstitute { .. } => None,
        }
    }

    /// Loads the Unicode of this font's codes for text extraction (9.10):
    /// the `ToUnicode` CMap of `font`, the dictionary this font was loaded
    /// from, and for a composite font of a character collection the
    /// provider's Unicode for it. Glyph selection never reads it.
    ///
    /// Strict mode rejects malformed ToUnicode data (9.10.3; ADR 0003).
    /// Repair mode reports repairs and retains unambiguous values, or
    /// discards an unusable map.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed ToUnicode data in Strict mode and
    /// errors from the provider.
    pub fn unicode(
        &self,
        document: &Document,
        font: &Dictionary,
        provider: Option<&dyn Provider>,
    ) -> Result<FontUnicode> {
        unicode::load(self, document, font, provider)
    }

    /// Splits a string operand into glyphs (clauses 9.4.3 and 9.7.6). With
    /// `unicode`, each glyph carries its Unicode text.
    ///
    /// # Errors
    ///
    /// Returns syntax errors for codes outside the font's code space that
    /// recovery cannot map, errors from loading glyph data, and with
    /// `unicode` a `Limit` error when the string's text exceeds the string
    /// budget.
    pub fn decode(&self, string: &[u8], unicode: Option<&FontUnicode>) -> Result<Vec<Glyph>> {
        let mut glyphs = Vec::new();
        self.decode_into(string, unicode, &mut glyphs)?;
        Ok(glyphs)
    }

    /// Decodes `string` into `glyphs`, replacing their contents, as
    /// [`Self::decode`] does. Without `unicode`, each glyph's
    /// [`Glyph::unicode`] is `None` and no text is looked up: rendering
    /// reuses one vector this way.
    ///
    /// # Errors
    ///
    /// As [`Self::decode`]; `glyphs` then holds an unspecified prefix.
    pub fn decode_into(
        &self,
        string: &[u8],
        unicode: Option<&FontUnicode>,
        glyphs: &mut Vec<Glyph>,
    ) -> Result<()> {
        glyphs.clear();
        self.decode_inner(string, unicode, glyphs)
            .map_err(|error| error.at(Position::Buffer(0)))
    }

    fn decode_inner(
        &self,
        string: &[u8],
        unicode: Option<&FontUnicode>,
        glyphs: &mut Vec<Glyph>,
    ) -> Result<()> {
        if !string.is_empty() && matches!(self.source, Source::MetricsOnly) {
            return Err(Error::unsupported(
                features::NON_EMBEDDED,
                "font metrics have no available outline program",
            ));
        }
        if string.len() > self.limits.string_bytes {
            return Err(Error::limit("text string byte budget exceeded"));
        }
        if let Mapping::Simple(simple) = &self.mapping {
            return self.decode_simple(simple, string, unicode, glyphs);
        }
        let mut offset = 0;
        let mut work = Meter::new(self.limits.font_work, "text decoding work budget exceeded");
        let mut text_bytes = Meter::new(
            self.limits.string_bytes,
            "decoded glyph text byte budget exceeded",
        );
        while let Some(input) = string.get(offset..).filter(|s| !s.is_empty()) {
            work.charge(1).map_err(|e| e.at(Position::Buffer(offset)))?;
            if glyphs.len() >= self.limits.text_items {
                return Err(
                    Error::limit("decoded glyph budget exceeded").at(Position::Buffer(offset))
                );
            }
            let result = self.decode_glyph(input, &mut work, unicode);
            let (glyph, text) = result.map_err(|e| e.at(Position::Buffer(offset)))?;
            text_bytes
                .charge(text)
                .map_err(|e| e.at(Position::Buffer(offset)))?;
            offset += usize::from(glyph.byte_len);
            glyphs.push(glyph);
        }
        Ok(())
    }

    /// [`Self::decode_inner`] for a simple font, one glyph per byte: the
    /// same glyphs, budgets and error positions, without the per-code
    /// dispatch.
    fn decode_simple(
        &self,
        simple: &SimpleMapping,
        string: &[u8],
        unicode: Option<&FontUnicode>,
        glyphs: &mut Vec<Glyph>,
    ) -> Result<()> {
        let mut work = Meter::new(self.limits.font_work, "text decoding work budget exceeded");
        let mut text_bytes = Meter::new(
            self.limits.string_bytes,
            "decoded glyph text byte budget exceeded",
        );
        glyphs.reserve(string.len().min(self.limits.text_items));
        for (offset, &byte) in string.iter().enumerate() {
            let at = Position::Buffer(offset);
            work.charge(1).map_err(|e| e.at(at))?;
            if glyphs.len() >= self.limits.text_items {
                return Err(Error::limit("decoded glyph budget exceeded").at(at));
            }
            let code = cmap::Code {
                value: u32::from(byte),
                len: 1,
            };
            let name = simple.names[usize::from(byte)].as_ref();
            let text = unicode
                .and_then(|unicode| unicode.text(code).or_else(|| simple.unicode(byte).cloned()));
            let shape = self
                .decode_shape(code, code.value, name, || {
                    simple.unicode(byte).map(GlyphText::as_str)
                })
                .map_err(|e| e.at(at))?;
            text_bytes
                .charge(text.as_deref().map_or(0, str::len))
                .map_err(|e| e.at(at))?;
            glyphs.push(Glyph {
                code: code.value,
                byte_len: 1,
                advance: self.metrics.width(code.value),
                vertical: (self.writing_mode == WritingMode::Vertical)
                    .then(|| self.metrics.vertical(code.value)),
                word_space: byte == 32,
                unicode: text,
                shape,
            });
        }
        Ok(())
    }

    /// One glyph and the byte length of its Unicode text, which it carries
    /// with `unicode`.
    fn decode_glyph(
        &self,
        input: &[u8],
        work: &mut Meter,
        unicode: Option<&FontUnicode>,
    ) -> Result<(Glyph, usize)> {
        let (code, selector, name, fallback) = match &self.mapping {
            Mapping::Simple(encoding) => {
                let byte = *input
                    .first()
                    .ok_or_else(|| Error::syntax("empty character code"))?;
                (
                    cmap::Code {
                        value: u32::from(byte),
                        len: 1,
                    },
                    u32::from(byte),
                    encoding.names[usize::from(byte)].as_ref(),
                    encoding.unicode(byte),
                )
            }
            Mapping::Composite { map, .. } => {
                let (code, in_codespace) = map.next(input, work)?;
                let mapped = if in_codespace {
                    map.cid(code, work)?
                } else {
                    None
                };
                // Collection Unicode belongs to the CID the code maps to, not
                // to a notdef substitute (9.10.2).
                let collection = mapped.and_then(|cid| unicode?.collection(cid));
                let cid = self.select_cid(map, code, mapped, work)?;
                (code, u32::from(cid), None, collection)
            }
        };
        let text = unicode.and_then(|unicode| unicode.text(code).or_else(|| fallback.cloned()));
        let length = text.as_deref().map_or(0, str::len);
        let glyph = Glyph {
            code: code.value,
            byte_len: code.len,
            advance: self.metrics.width(selector),
            vertical: (self.writing_mode == WritingMode::Vertical)
                .then(|| self.metrics.vertical(selector)),
            word_space: code.len == 1 && code.value == 32,
            unicode: text,
            shape: self.decode_shape(code, selector, name, || fallback.map(GlyphText::as_str))?,
        };
        Ok((glyph, length))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{dictionary, document};
    use pagura_core::ErrorKind;

    #[test]
    fn decode_limits_and_positions() -> Result<()> {
        let document = document("type3-differences")?;
        let mut font = PdfFont::load(&document, dictionary(&document)?)?;
        let unicode = font.unicode(&document, dictionary(&document)?, None)?;
        let text = Some(&unicode);
        font.limits.string_bytes = 1;
        assert_eq!(
            font.decode(b"AA", text).err().map(|e| e.kind),
            Some(ErrorKind::Limit)
        );
        font.limits.string_bytes = 100;
        font.limits.text_items = 1;
        assert_eq!(
            font.decode(b"AA", text).err().map(|e| e.position),
            Some(Position::Buffer(1))
        );
        // The glyph text of one string is bounded like a decoded string;
        // without Unicode no text is looked up.
        font.limits.text_items = 100;
        font.limits.string_bytes = 1;
        assert_eq!(
            font.decode(b"C", text).err().map(|e| e.kind),
            Some(ErrorKind::Limit)
        );
        assert!(font.decode(b"C", None).is_ok());
        font.limits.font_work = 0;
        assert_eq!(
            font.decode(b"A", text).err().map(|e| e.kind),
            Some(ErrorKind::Limit)
        );
        Ok(())
    }
}
