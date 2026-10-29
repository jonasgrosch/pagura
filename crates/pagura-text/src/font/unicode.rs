//! ToUnicode maps and collection Unicode of a loaded font (9.10), loaded
//! only for text extraction.
use super::{FontKind, Mapping, PdfFont, Source};
use crate::{
    CidToUnicode, GlyphText,
    cid::Cid,
    cmap::{self, CMap, CMapRole, Code},
    objects::{optional, required},
};
use pagura_core::{Dictionary, Error, Position, Result, Value};
use pagura_document::Document;
use std::sync::Arc;

/// The Unicode of a font's character codes (9.10.2): its `ToUnicode` CMap,
/// and the Unicode of its character collection for composite fonts whose
/// collection the provider knows. Codes without either fall back to their
/// glyph names. Load it with [`PdfFont::unicode`]; rendering never does.
#[derive(Debug, Clone)]
pub struct FontUnicode {
    to_unicode: Option<Arc<CMap>>,
    collection: Option<Arc<CidToUnicode>>,
    retained: usize,
}

impl FontUnicode {
    pub(crate) fn new(
        to_unicode: Option<Arc<CMap>>,
        collection: Option<Arc<CidToUnicode>>,
    ) -> Self {
        let retained = size_of::<Self>().saturating_add(to_unicode.as_ref().map_or(0, |map| {
            size_of_val(map.as_ref())
                .saturating_add(2 * size_of::<usize>())
                .saturating_add(map.heap_bytes())
        }));
        Self {
            to_unicode,
            collection,
            retained,
        }
    }

    /// Estimates retained storage as [`PdfFont::retained_bytes`] does: the
    /// ToUnicode CMap; collection tables belong to the provider.
    pub fn retained_bytes(&self) -> usize {
        self.retained
    }

    /// The ToUnicode text of `code`.
    pub(crate) fn text(&self, code: Code) -> Option<GlyphText> {
        self.to_unicode.as_ref()?.text(code)
    }

    /// The collection's Unicode for `cid`.
    pub(crate) fn collection(&self, cid: Cid) -> Option<&GlyphText> {
        self.collection.as_ref()?.text(cid)
    }
}

/// Loads the Unicode of `font`, loaded from the dictionary `dictionary`.
pub(super) fn load(
    font: &PdfFont,
    document: &Document,
    dictionary: &Dictionary,
    provider: Option<&dyn crate::Provider>,
) -> Result<FontUnicode> {
    let to_unicode = to_unicode(document, dictionary, &font.mapping)?;
    let collection = match &font.source {
        Source::CidSubstitute { unicode, .. } => Some(Arc::clone(unicode)),
        Source::Program { .. } if font.kind == FontKind::Type0 => {
            let (descendant, _) = super::load::descendant(document, dictionary)?;
            collection_unicode(document, descendant, &font.mapping, provider)?
        }
        Source::Program { .. }
        | Source::UnicodeSubstitute { .. }
        | Source::Type3 { .. }
        | Source::MetricsOnly => None,
    };
    Ok(FontUnicode::new(to_unicode, collection))
}

pub(crate) fn to_unicode(
    document: &Document,
    font: &Dictionary,
    mapping: &Mapping,
) -> Result<Option<Arc<CMap>>> {
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
) -> Result<Option<Arc<CMap>>> {
    to_unicode_inner(document, value, mapping).map_err(|mut error| {
        error
            .repair
            .get_or_insert(crate::repairs::TOUNICODE_INVALID);
        error
    })
}

fn to_unicode_inner(
    document: &Document,
    value: &Value,
    mapping: &Mapping,
) -> Result<Option<Arc<CMap>>> {
    // Null dictionary values are absent, including indirect or undefined ones
    // (7.3.9-7.3.10). They require no repair in either recovery mode.
    if document.resolve(value)?.is_null() {
        return Ok(None);
    }
    let id = value
        .as_reference()
        .ok_or_else(|| Error::syntax("ToUnicode must be a stream reference"))?;
    let map = cmap::load(document, value, CMapRole::ToUnicode, None)?;
    let start = Position::Stream {
        object: id,
        offset: 0,
    };
    let map = match mapping {
        Mapping::Simple(_) => map
            .simple_compatible(document, start)
            .map_err(|e| e.in_object(id))?
            .map_or(map, Arc::new),
        Mapping::Composite { map: encoding, .. } => {
            map.consistent_with(encoding, &document.limits())
                .map_err(|e| e.in_object(id))?;
            map
        }
    };
    Ok(Some(map))
}

fn collection_unicode(
    document: &Document,
    font: &Dictionary,
    mapping: &Mapping,
    provider: Option<&dyn crate::Provider>,
) -> Result<Option<Arc<CidToUnicode>>> {
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
        && let Some(collection) = map.collection()
    {
        return Ok(Some(collection));
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
