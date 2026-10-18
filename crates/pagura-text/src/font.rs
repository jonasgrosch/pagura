//! Loaded PDF fonts (9.6–9.7): the public font type and string decoding.
use crate::{
    CidToUnicode, Glyph, Provider,
    cid::{self, CidToGidMap},
    cmap::{self, CMap},
    encoding::SimpleMapping,
    features, memory, metrics,
    selection::Route,
};
use pagura_core::{Dictionary, Error, Limits, Name, ObjectId, Position, Result};
use pagura_document::Document;
use pagura_font::FontProgram;
use pagura_raster::Transform;
use std::{collections::BTreeMap, sync::Arc};

mod load;
mod shape;
mod unicode;
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

/// A loaded PDF font. Load each font dictionary once per document and share it.
#[derive(Debug)]
pub struct PdfFont {
    pub(crate) kind: FontKind,
    pub(crate) writing_mode: WritingMode,
    pub(crate) source: Source,
    pub(crate) mapping: Mapping,
    pub(crate) metrics: metrics::Metrics,
    pub(crate) to_unicode: Option<CMap>,
    pub(crate) limits: Limits,
    pub(crate) cid_unicode: Option<Arc<CidToUnicode>>,
    pub(crate) retained: usize,
}

impl PdfFont {
    /// Estimates retained storage, including this font and its owned allocations
    /// (ADR 0012). Computed once at load; this accessor is constant time.
    ///
    /// Counts encoding payloads, width/vertical tables, owned CMaps, ToUnicode,
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
    /// Loads the font dictionary `font` (already resolved) of `document`.
    ///
    /// Strict mode rejects malformed ToUnicode data (9.10.3; ADR 0003).
    /// Repair mode reports repairs and retains unambiguous values, or discards
    /// an unusable map without affecting glyph selection.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed dictionaries or ToUnicode data in Strict mode, `Unsupported`
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

    /// The writing mode.
    pub fn writing_mode(&self) -> WritingMode {
        self.writing_mode
    }

    /// The outline program that draws this font's glyphs: embedded, or a
    /// provider's substitute. `None` for Type 3 fonts and for fonts whose
    /// provider supplies metrics only.
    pub fn program(&self) -> Option<&Arc<FontProgram>> {
        match &self.source {
            Source::Program { program, .. } | Source::CidSubstitute { program } => Some(program),
            Source::Type3 { .. } | Source::MetricsOnly => None,
        }
    }

    /// The `/Resources` of a Type 3 font, for its glyph procedures.
    pub fn type3_resources(&self) -> Option<&Dictionary> {
        match &self.source {
            Source::Type3 { resources, .. } => resources.as_ref(),
            Source::Program { .. } | Source::MetricsOnly | Source::CidSubstitute { .. } => None,
        }
    }

    /// Splits a string operand into glyphs (clauses 9.4.3 and 9.7.6).
    ///
    /// # Errors
    ///
    /// Returns syntax errors for codes outside the font's code space that
    /// recovery cannot map, and errors from loading glyph data.
    pub fn decode(&self, string: &[u8]) -> Result<Vec<Glyph>> {
        let mut glyphs = Vec::new();
        self.decode_into(string, true, &mut glyphs)?;
        Ok(glyphs)
    }

    /// Decodes `string` into `glyphs`, replacing their contents, as
    /// [`Self::decode`] does. Without `unicode`, each glyph's
    /// [`Glyph::unicode`] is `None`; its text is still charged to the
    /// decoded-byte budget, so results and errors are otherwise those of
    /// `decode`. Rendering reuses one vector this way and copies no text.
    ///
    /// # Errors
    ///
    /// As [`Self::decode`]; `glyphs` then holds an unspecified prefix.
    pub fn decode_into(&self, string: &[u8], unicode: bool, glyphs: &mut Vec<Glyph>) -> Result<()> {
        glyphs.clear();
        self.decode_inner(string, unicode, glyphs)
            .map_err(|error| error.at(Position::Buffer(0)))
    }

    fn decode_inner(&self, string: &[u8], unicode: bool, glyphs: &mut Vec<Glyph>) -> Result<()> {
        if !string.is_empty() && matches!(self.source, Source::MetricsOnly) {
            return Err(Error::unsupported(
                features::NON_EMBEDDED,
                "font metrics have no available outline program",
            ));
        }
        if string.len() > self.limits.string_bytes {
            return Err(Error::limit("text string byte budget exceeded"));
        }
        let mut offset = 0;
        let mut work = 0;
        let mut unicode_bytes = 0;
        while let Some(input) = string.get(offset..).filter(|s| !s.is_empty()) {
            cmap::charge(&mut work, 1, self.limits.operations)
                .map_err(|e| e.at(Position::Buffer(offset)))?;
            if glyphs.len() >= self.limits.value_nodes {
                return Err(
                    Error::limit("decoded glyph budget exceeded").at(Position::Buffer(offset))
                );
            }
            let result = self.decode_glyph(input, &mut work, unicode);
            let (glyph, text) = result.map_err(|e| e.at(Position::Buffer(offset)))?;
            cmap::charge(&mut unicode_bytes, text, self.limits.decoded_bytes)
                .map_err(|e| e.at(Position::Buffer(offset)))?;
            offset += usize::from(glyph.byte_len);
            glyphs.push(glyph);
        }
        Ok(())
    }

    /// One glyph and the byte length of its Unicode text, which the glyph
    /// carries only with `unicode`.
    fn decode_glyph(
        &self,
        input: &[u8],
        work: &mut usize,
        unicode: bool,
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
                    encoding.unicode[usize::from(byte)].as_deref(),
                )
            }
            Mapping::Composite { map, .. } => {
                let (code, valid) = map.next(input, work, self.limits)?;
                let mut cid = if valid {
                    map.cid(code, work, self.limits)?
                } else {
                    None
                };
                let unicode = cid.and_then(|cid| self.cid_unicode.as_ref()?.get(u32::from(cid)));
                if cid.is_some_and(|cid| self.lacks_gid_entry(cid)) {
                    cid = Some(cid::Cid::NOTDEF);
                } else if cid.is_none_or(|cid| !self.has_cid(cid)) {
                    cid = Some(map.notdef(code, work, self.limits)?);
                }
                let cid = cid
                    .filter(|cid| self.has_cid(*cid))
                    .unwrap_or(cid::Cid::NOTDEF);
                (code, u32::from(cid), None, unicode)
            }
        };
        let text = self
            .to_unicode
            .as_ref()
            .and_then(|map| map.text(code))
            .or(fallback);
        let length = text.map_or(0, str::len);
        let unicode = text.filter(|_| unicode).map(str::to_owned);
        let glyph = Glyph {
            code: code.value,
            byte_len: code.len,
            advance: self.metrics.width(selector),
            vertical: (self.writing_mode == WritingMode::Vertical)
                .then(|| self.metrics.vertical(selector)),
            word_space: code.len == 1 && code.value == 32,
            unicode,
            shape: self.shape(code, selector, name, fallback),
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
        font.limits.string_bytes = 1;
        assert_eq!(
            font.decode(b"AA").err().map(|e| e.kind),
            Some(ErrorKind::Limit)
        );
        font.limits.string_bytes = 100;
        font.limits.value_nodes = 1;
        assert_eq!(
            font.decode(b"AA").err().map(|e| e.position),
            Some(Position::Buffer(1))
        );
        font.limits.value_nodes = 100;
        font.limits.decoded_bytes = 1;
        assert_eq!(
            font.decode(b"C").err().map(|e| e.kind),
            Some(ErrorKind::Limit)
        );
        font.limits.operations = 0;
        assert_eq!(
            font.decode(b"A").err().map(|e| e.kind),
            Some(ErrorKind::Limit)
        );
        Ok(())
    }
}
