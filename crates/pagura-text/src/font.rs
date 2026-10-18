//! PDF font loading and glyph selection (9.6–9.7).
use crate::{
    FontKind, GlyphShape, PdfFont, WritingMode,
    cmap::{self, CMap, Code},
    encoding::{self, SimpleMapping},
    features,
    metrics::Metrics,
    objects::{array, optional, required},
};
use pagura_core::{Diagnostic, Dictionary, Error, Name, ObjectId, Position, Result, Value};
use pagura_document::Document;
use pagura_font::{FontProgram, GlyphId};
use pagura_raster::Transform;
use std::{collections::BTreeMap, sync::Arc};

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
        truetype_raw: bool,
        substituted: bool,
        cid_map: Option<Vec<u8>>,
    },
}

#[derive(Debug)]
pub(crate) enum Mapping {
    Simple(Box<SimpleMapping>),
    Composite { map: Arc<CMap>, shared: bool },
}

pub(crate) fn load(document: &Document, font: &Dictionary) -> Result<PdfFont> {
    load_with(document, font, None, false)
}

/// With `rendering`, a TrueType program is decoded only as far as drawing
/// reads it ([`PdfFont::load_for_rendering`]).
pub(crate) fn load_with(
    document: &Document,
    font: &Dictionary,
    provider: Option<&dyn crate::Provider>,
    rendering: bool,
) -> Result<PdfFont> {
    let kind = kind(document, font)?;
    if kind == FontKind::Type3 {
        return type3(document, font);
    }
    let (metrics_font, mapping, metrics_id) = if kind == FontKind::Type0 {
        let descendants = array(required(document, font, b"DescendantFonts")?)?;
        if descendants.len() != 1 {
            return Err(Error::syntax("Type0 requires one descendant font"));
        }
        let descendant = document.resolve_dictionary(&descendants[0])?;
        if !descendant.is_name(b"Subtype", b"CIDFontType0")
            && !descendant.is_name(b"Subtype", b"CIDFontType2")
        {
            return Err(Error::syntax("invalid CIDFont subtype"));
        }
        (
            descendant,
            Some(composite_mapping(document, font, provider)?),
            descendants[0].as_reference(),
        )
    } else {
        (font, None, None)
    };
    let descriptor = optional(document, metrics_font, b"FontDescriptor")?
        .map(|v| document.resolve_dictionary(v))
        .transpose()?;
    let embedded = descriptor.is_some_and(|dict| {
        [b"FontFile".as_slice(), b"FontFile2", b"FontFile3"]
            .iter()
            .any(|key| dict.contains_key(key))
    });
    if !embedded && let Some(provider) = provider {
        if let Some(mapping) = mapping {
            return crate::cid_substitute::load(
                document,
                font,
                metrics_font,
                metrics_id,
                mapping,
                provider,
            );
        }
        return crate::substitute::load(document, font, descriptor, kind, provider);
    }
    let descriptor = descriptor.ok_or_else(|| {
        Error::unsupported(features::NON_EMBEDDED, "font program is not embedded")
    })?;
    let symbolic = symbolic(document, descriptor)?;
    let program = Arc::new(crate::program::load(
        document,
        descriptor,
        kind,
        metrics_font,
        rendering,
    )?);
    let metrics = if kind == FontKind::Type0 {
        Metrics::composite(document, metrics_font)?
    } else {
        Metrics::simple(document, font, Some(descriptor), 0.001)?
    };
    let mapping = if let Some(mapping) = mapping {
        mapping
    } else {
        Mapping::Simple(Box::new(simple_mapping(
            document,
            font,
            kind,
            symbolic,
            program.builtin_encoding(),
        )?))
    };
    let cid_map = cid_map(document, metrics_font, metrics_id)?;
    let mode = match &mapping {
        Mapping::Composite { map, .. } => map.mode,
        Mapping::Simple(_) => WritingMode::Horizontal,
    };
    let to_unicode = to_unicode(document, font, &mapping)?;
    let cid_unicode = collection_unicode(document, metrics_font, &mapping, provider)?;
    Ok(PdfFont {
        kind,
        writing_mode: mode,
        source: Source::Program {
            program,
            truetype_raw: symbolic || optional(document, font, b"Encoding")?.is_none(),
            substituted: false,
            cid_map,
        },
        mapping,
        metrics,
        to_unicode,
        limits: document.limits(),
        cid_unicode,
        retained: 0,
    }
    .accounted())
}

fn kind(document: &Document, font: &Dictionary) -> Result<FontKind> {
    Ok(
        match required(document, font, b"Subtype")?
            .as_name()
            .map(Name::as_bytes)
        {
            Some(b"Type1") => FontKind::Type1,
            Some(b"MMType1") => FontKind::MmType1,
            Some(b"TrueType") => FontKind::TrueType,
            Some(b"Type3") => FontKind::Type3,
            Some(b"Type0") => FontKind::Type0,
            _ => return Err(Error::syntax("invalid font Subtype")),
        },
    )
}

fn composite_mapping(
    document: &Document,
    font: &Dictionary,
    provider: Option<&dyn crate::Provider>,
) -> Result<Mapping> {
    let encoding = font
        .get(b"Encoding")
        .ok_or_else(|| Error::syntax("Type0 missing Encoding"))?;
    let shared = document
        .resolve(encoding)?
        .as_name()
        .is_some_and(|name| !matches!(name.as_bytes(), b"Identity-H" | b"Identity-V"));
    Ok(Mapping::Composite {
        map: cmap::load_with(document, encoding, false, provider)?,
        shared,
    })
}

fn symbolic(document: &Document, descriptor: &Dictionary) -> Result<bool> {
    let flags = optional(document, descriptor, b"Flags")?
        .and_then(Value::as_integer)
        .ok_or_else(|| Error::syntax("invalid font descriptor Flags"))?;
    let symbolic = flags & 4 != 0;
    if symbolic == (flags & 32 != 0) {
        return Err(Error::syntax(
            "font must set exactly one of Symbolic and Nonsymbolic",
        ));
    }
    Ok(symbolic)
}

fn simple_mapping(
    document: &Document,
    font: &Dictionary,
    kind: FontKind,
    symbolic: bool,
    builtin: Option<&encoding::Encoding>,
) -> Result<SimpleMapping> {
    if kind == FontKind::TrueType && (symbolic || optional(document, font, b"Encoding")?.is_none())
    {
        return SimpleMapping::new(std::array::from_fn(|_| None), document.limits(), b"");
    }
    let default = if kind == FontKind::TrueType {
        encoding::base(b"StandardEncoding")?
    } else {
        builtin
            .cloned()
            .unwrap_or(encoding::base(b"StandardEncoding")?)
    };
    let mut names = encoding::load(document, font.get(b"Encoding"), default)?;
    if kind == FontKind::TrueType
        && optional(document, font, b"Encoding")?
            .is_some_and(|value| value.as_dictionary().is_some())
    {
        for (slot, fallback) in names.iter_mut().zip(encoding::base(b"StandardEncoding")?) {
            if slot.is_none() {
                *slot = fallback;
            }
        }
    }
    SimpleMapping::new(
        names,
        document.limits(),
        encoding::font_name(document, font)?,
    )
}

fn type3(document: &Document, font: &Dictionary) -> Result<PdfFont> {
    let values = array(required(document, font, b"FontMatrix")?)?;
    if values.len() != 6 {
        return Err(Error::syntax("FontMatrix must have six numbers"));
    }
    let mut matrix = [0.0; 6];
    for (slot, value) in matrix.iter_mut().zip(values) {
        *slot = document
            .resolve(value)?
            .as_number()
            .ok_or_else(|| Error::syntax("invalid FontMatrix component"))?;
    }
    let matrix = Transform::new(
        matrix[0], matrix[1], matrix[2], matrix[3], matrix[4], matrix[5],
    );
    let bbox = array(required(document, font, b"FontBBox")?)?;
    if bbox.len() != 4
        || bbox.iter().any(|v| {
            document
                .resolve(v)
                .map_or(true, |v| v.as_number().is_none())
        })
    {
        return Err(Error::syntax("invalid Type3 FontBBox"));
    }
    let procs = document.resolve_dictionary(required(document, font, b"CharProcs")?)?;
    let mut procedures = BTreeMap::new();
    for (name, value) in procs {
        let id = value
            .as_reference()
            .ok_or_else(|| Error::syntax("CharProcs entry must be a stream reference"))?;
        if !document.object(id)?.is_stream() {
            return Err(Error::syntax("CharProcs entry is not a stream").in_object(id));
        }
        procedures.insert(name.clone(), id);
    }
    let resources = optional(document, font, b"Resources")?
        .map(|v| document.resolve_dictionary(v).cloned())
        .transpose()?;
    let encoding = required(document, font, b"Encoding")?;
    let encoding_dict = encoding
        .as_dictionary()
        .ok_or_else(|| Error::syntax("Type3 Encoding must be a dictionary"))?;
    if !encoding_dict.contains_key(b"Differences") {
        return Err(Error::syntax("Type3 Encoding missing Differences"));
    }
    let encoding = encoding::load(document, Some(encoding), std::array::from_fn(|_| None))?;
    let mapping = Mapping::Simple(Box::new(SimpleMapping::new(
        encoding,
        document.limits(),
        encoding::font_name(document, font)?,
    )?));
    let to_unicode = to_unicode(document, font, &mapping)?;
    Ok(PdfFont {
        kind: FontKind::Type3,
        writing_mode: WritingMode::Horizontal,
        metrics: Metrics::simple(document, font, None, matrix.a)?,
        source: Source::Type3 {
            procedures,
            resources,
            matrix,
        },
        mapping,
        to_unicode,
        limits: document.limits(),
        cid_unicode: None,
        retained: 0,
    }
    .accounted())
}

pub(crate) fn to_unicode(
    document: &Document,
    font: &Dictionary,
    mapping: &Mapping,
) -> Result<Option<CMap>> {
    let Some(value) = font.get(b"ToUnicode") else {
        return Ok(None);
    };
    match to_unicode_map(document, value, mapping) {
        Ok(map) => Ok(map),
        Err(error) => {
            crate::report::Report::new(document).discarded(error, value.as_reference())?;
            Ok(None)
        }
    }
}

// ToUnicode does not select glyphs (9.10.3), but its malformed data still
// requires permission to repair or discard (ADR 0003).
pub(crate) fn to_unicode_map(
    document: &Document,
    value: &Value,
    mapping: &Mapping,
) -> Result<Option<CMap>> {
    to_unicode_inner(document, value, mapping).map_err(|mut error| {
        error
            .repair
            .get_or_insert(crate::repairs::TOUNICODE_INVALID);
        error
    })
}

fn to_unicode_inner(document: &Document, value: &Value, mapping: &Mapping) -> Result<Option<CMap>> {
    // Null dictionary values are absent, including indirect or undefined ones
    // (7.3.9-7.3.10). They require no repair in either recovery mode.
    if document.resolve(value)?.is_null() {
        return Ok(None);
    }
    let id = value
        .as_reference()
        .ok_or_else(|| Error::syntax("ToUnicode must be a stream reference"))?;
    let mut map = cmap::load(document, value, true)?;
    match mapping {
        Mapping::Simple(_) => map.simple_compatible(
            document,
            Position::Stream {
                object: id,
                offset: 0,
            },
        ),
        Mapping::Composite { map: encoding, .. } => {
            map.consistent_with(encoding, document.limits())
        }
    }
    .map_err(|e| e.in_object(id))?;
    Ok(Some(map))
}

fn collection_unicode(
    document: &Document,
    font: &Dictionary,
    mapping: &Mapping,
    provider: Option<&dyn crate::Provider>,
) -> Result<Option<Arc<crate::CidToUnicode>>> {
    let (Mapping::Composite { .. }, Some(provider)) = (mapping, provider) else {
        return Ok(None);
    };
    let Some((registry, ordering)) = collection(document, font, mapping)? else {
        return Ok(None);
    };
    provider.cid_to_unicode(registry, ordering)
}

fn collection<'a>(
    document: &'a Document,
    font: &'a Dictionary,
    mapping: &'a Mapping,
) -> Result<Option<(&'a [u8], &'a [u8])>> {
    if let Mapping::Composite { map, .. } = mapping
        && let (Some(registry), Some(ordering)) = (&map.registry, &map.ordering)
    {
        return Ok(Some((registry.as_bytes(), ordering.as_bytes())));
    }
    cid_collection(document, font)
}

pub(crate) fn cid_collection<'a>(
    document: &'a Document,
    font: &'a Dictionary,
) -> Result<Option<(&'a [u8], &'a [u8])>> {
    let Some(info) = optional(document, font, b"CIDSystemInfo")? else {
        return Ok(None);
    };
    let info = document.resolve_dictionary(info)?;
    let registry = required(document, info, b"Registry")?
        .as_string()
        .ok_or_else(|| Error::syntax("invalid CID registry"))?;
    let ordering = required(document, info, b"Ordering")?
        .as_string()
        .ok_or_else(|| Error::syntax("invalid CID ordering"))?;
    Ok(Some((registry, ordering)))
}

fn stream(document: &Document, value: &Value) -> Result<Vec<u8>> {
    let id = value
        .as_reference()
        .ok_or_else(|| Error::syntax("font stream must be indirect"))?;
    document
        .stream_data(document.object(id)?)
        .map(std::borrow::Cow::into_owned)
        .map_err(|e| e.in_object(id))
}

impl PdfFont {
    pub(crate) fn shape(
        &self,
        code: Code,
        selector: u32,
        name: Option<&Name>,
        unicode: Option<&str>,
    ) -> GlyphShape {
        match &self.source {
            Source::MetricsOnly => GlyphShape::Blank,
            Source::CidSubstitute { program } => {
                crate::cid_substitute::glyph(program, self.cid_unicode.as_deref(), selector).map_or(
                    GlyphShape::Blank,
                    |glyph| GlyphShape::Outline {
                        program: Arc::clone(program),
                        glyph,
                        matrix: program.font_matrix(),
                    },
                )
            }
            Source::Type3 {
                procedures, matrix, ..
            } => name
                .and_then(|name| procedures.get(name))
                .map_or(GlyphShape::Blank, |id| GlyphShape::Type3 {
                    procedure: *id,
                    matrix: *matrix,
                }),
            Source::Program {
                program,
                truetype_raw,
                substituted,
                cid_map,
            } => {
                let select = || {
                    if (self.kind == FontKind::TrueType || *substituted)
                        && program.format() != pagura_font::FontFormat::Cff
                    {
                        crate::selection::truetype(
                            program,
                            code.value,
                            name,
                            unicode,
                            *truetype_raw,
                        )
                    } else {
                        name.and_then(|name| program.glyph_for_name(name.as_bytes()))
                    }
                };
                let glyph = if self.kind == FontKind::Type0 {
                    cid_glyph(program, cid_map.as_deref(), selector)
                } else if let Mapping::Simple(simple) = &self.mapping
                    && code.len == 1
                    && let Some(slot) = usize::try_from(code.value)
                        .ok()
                        .and_then(|c| simple.selected.get(c))
                {
                    *slot.get_or_init(select)
                } else {
                    select()
                };
                GlyphShape::Outline {
                    program: Arc::clone(program),
                    glyph: glyph.unwrap_or(GlyphId(0)),
                    matrix: program.font_matrix(),
                }
            }
        }
    }

    // 9.7.6.3 specifically requires CID 0 when a CIDToGIDMap stream
    // has no entry, before considering ordinary CMap notdef substitution.
    pub(crate) fn lacks_gid_entry(&self, cid: u32) -> bool {
        match &self.source {
            Source::Program {
                cid_map: Some(map), ..
            } => usize::try_from(cid)
                .ok()
                .and_then(|cid| cid.checked_mul(2))
                .is_none_or(|offset| offset >= map.len()),
            _ => false,
        }
    }

    pub(crate) fn has_cid(&self, cid: u32) -> bool {
        match &self.source {
            Source::Program {
                program, cid_map, ..
            } => cid_glyph(program, cid_map.as_deref(), cid).is_some(),
            Source::CidSubstitute { .. } => cid <= 65535,
            Source::Type3 { .. } | Source::MetricsOnly => false,
        }
    }
}

fn cid_glyph(program: &FontProgram, map: Option<&[u8]>, cid: u32) -> Option<GlyphId> {
    if program.is_cid_keyed() {
        return program.glyph_for_cid(cid);
    }
    if map.is_some_and(|map| {
        usize::try_from(cid)
            .ok()
            .is_none_or(|cid| cid >= map.len() / 2)
    }) {
        return None;
    }
    let glyph = mapped_gid(map, cid);
    (glyph < program.glyph_count()).then_some(GlyphId(glyph))
}

fn cid_map(
    document: &Document,
    metrics_font: &Dictionary,
    id: Option<ObjectId>,
) -> Result<Option<Vec<u8>>> {
    Ok(if metrics_font.is_name(b"Subtype", b"CIDFontType2") {
        match optional(document, metrics_font, b"CIDToGIDMap")? {
            None => {
                crate::report::Report::new(document).diagnostic(
                    Diagnostic::new(
                        crate::repairs::CID_TO_GID_IDENTITY,
                        "embedded CIDFontType2 missing CIDToGIDMap; using Identity (9.7.4.1, Table 115)",
                    )
                    .in_object_opt(id),
                )?;
                None
            }
            Some(Value::Name(name)) if name == b"Identity" => None,
            Some(_) => {
                let bytes = stream(
                    document,
                    metrics_font
                        .get(b"CIDToGIDMap")
                        .ok_or_else(|| Error::syntax("missing CIDToGIDMap"))?,
                )?;
                if bytes.len() % 2 != 0 || bytes.len() > 131_072 {
                    return Err(Error::syntax("invalid CIDToGIDMap length"));
                }
                Some(bytes)
            }
        }
    } else {
        None
    })
}

fn mapped_gid(map: Option<&[u8]>, cid: u32) -> u32 {
    let Some(map) = map else { return cid };
    let Some(offset) = usize::try_from(cid).ok().and_then(|cid| cid.checked_mul(2)) else {
        return 0;
    };
    let Some(bytes) = offset.checked_add(2).and_then(|end| map.get(offset..end)) else {
        return 0;
    };
    u32::from(u16::from_be_bytes([bytes[0], bytes[1]]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pagura_core::syntax::{Origin, Parser};
    use pagura_document::OpenOptions;
    use pagura_testkit::fixture;

    #[test]
    fn cid_to_gid_vector_and_identity() -> Result<()> {
        let document = Document::open(fixture("text", "cid-to-gid.pdf"), &OpenOptions::default())?;
        let dict = document
            .object(ObjectId::new(4, 0))?
            .dictionary()
            .ok_or_else(|| Error::syntax("missing dictionary"))?;
        let map = cid_map(&document, dict, Some(ObjectId::new(4, 0)))?;
        let data = fixture("text", "cid-to-gid.out");
        let expected = Parser::new(&data, Origin::Buffer, document.limits()).parse()?;
        for value in array(&expected)? {
            let values = array(value)?;
            let code = values[0]
                .as_integer()
                .and_then(|v| u32::try_from(v).ok())
                .ok_or_else(|| Error::syntax("invalid oracle code"))?;
            assert_eq!(
                i64::from(mapped_gid(map.as_deref(), code)),
                values[1]
                    .as_integer()
                    .ok_or_else(|| Error::syntax("invalid oracle glyph"))?
            );
            assert_eq!(mapped_gid(None, code), code);
        }
        Ok(())
    }
    #[test]
    fn symbolic_truetype_ignores_encoding_and_nonsymbolic_uses_names() -> Result<()> {
        let document = Document::open(
            fixture("text", "type3-differences.pdf"),
            &OpenOptions::default(),
        )?;
        let font: Dictionary = [(Name::new(b"Encoding"), Value::name(b"InvalidEncoding"))]
            .into_iter()
            .collect();
        let mapping = simple_mapping(&document, &font, FontKind::TrueType, true, None)?;
        assert!(mapping.names.iter().all(Option::is_none));
        assert!(mapping.unicode.iter().all(Option::is_none));
        assert!(simple_mapping(&document, &font, FontKind::TrueType, false, None).is_err());
        let mapping = simple_mapping(
            &document,
            &Dictionary::new(),
            FontKind::TrueType,
            false,
            None,
        )?;
        assert!(mapping.names.iter().all(Option::is_none));
        let named: Dictionary = [(Name::new(b"Encoding"), Value::name(b"WinAnsiEncoding"))]
            .into_iter()
            .collect();
        let mapping = simple_mapping(&document, &named, FontKind::TrueType, false, None)?;
        assert_eq!(mapping.unicode[65].as_deref(), Some("A"));
        Ok(())
    }
}
