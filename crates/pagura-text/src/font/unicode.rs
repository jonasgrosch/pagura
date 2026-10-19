//! ToUnicode maps and collection Unicode of a loaded font (9.10).
use super::Mapping;
use crate::{
    cmap::{self, CMap},
    objects::{optional, required},
};
use pagura_core::{Dictionary, Error, Position, Result, Value};
use pagura_document::Document;
use std::sync::Arc;

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

pub(super) fn collection_unicode(
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
