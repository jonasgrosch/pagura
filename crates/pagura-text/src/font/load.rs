//! Font dictionary loading (9.6–9.7; Tables 109–117).
use super::{
    FontKind, Mapping, Ownership, PdfFont, Source, WritingMode, to_unicode,
    unicode::collection_unicode,
};
use crate::{
    cid::CidToGidMap,
    cmap,
    descriptor::{Descriptor, FontFlags},
    encoding::{self, SimpleMapping},
    features,
    metrics::{GLYPH_SPACE_UNITS, Metrics},
    objects::{array, optional, required},
    selection::Route,
};
use pagura_core::{Diagnostic, Dictionary, Error, ObjectId, Position, Result, Value};
use pagura_document::Document;
use pagura_raster::Transform;
use std::{collections::BTreeMap, sync::Arc};

pub(super) fn load(document: &Document, font: &Dictionary) -> Result<PdfFont> {
    load_with(document, font, None, Position::Unknown, false)
}

/// Loads `font`; `position` locates the descriptor-flags repair. With
/// `rendering`, a TrueType program is decoded only as far as drawing reads
/// it ([`PdfFont::load_for_rendering`]).
pub(super) fn load_with(
    document: &Document,
    font: &Dictionary,
    provider: Option<&dyn crate::Provider>,
    position: Position,
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
    let descriptor = Descriptor::of(document, metrics_font)?;
    // One predicate decides embedding for substitution and program loading,
    // so a null or dangling FontFile still reaches the provider (7.3.10).
    let embedded = match descriptor {
        Some(descriptor) => descriptor.program_key(document)?.is_some(),
        None => false,
    };
    if !embedded && let Some(provider) = provider {
        if let Some(mapping) = mapping {
            // CID substitution ignores the flags, but a contradictory pair is
            // still the descriptor's reported repair.
            if let Some(descriptor) = descriptor {
                descriptor.normalized_flags(document, position)?;
            }
            return crate::substitute::cid::load(
                document,
                font,
                metrics_font,
                metrics_id,
                mapping,
                provider,
            );
        }
        return crate::substitute::load(document, font, descriptor, kind, provider, position);
    }
    let descriptor = descriptor.ok_or_else(|| {
        Error::unsupported(features::NON_EMBEDDED, "font program is not embedded")
    })?;
    let symbolic = descriptor
        .normalized_flags(document, position)?
        .contains(FontFlags::SYMBOLIC);
    // 9.6.5.4: symbolic TrueType fonts and fonts without an Encoding use
    // their bytes as character codes.
    let route = if symbolic || optional(document, font, b"Encoding")?.is_none() {
        Route::Raw
    } else {
        Route::Named
    };
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
        Metrics::simple(
            document,
            font,
            Some(descriptor.dictionary),
            GLYPH_SPACE_UNITS,
        )?
    };
    let mapping = if let Some(mapping) = mapping {
        mapping
    } else {
        Mapping::Simple(Box::new(simple_mapping(
            document,
            font,
            kind,
            route,
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
            route,
            ownership: Ownership::Embedded,
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
    required(document, font, b"Subtype")?
        .as_name()
        .and_then(|name| FontKind::from_subtype(name.as_bytes()))
        .ok_or(Error::syntax("invalid font Subtype"))
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

fn simple_mapping(
    document: &Document,
    font: &Dictionary,
    kind: FontKind,
    route: Route,
    builtin: Option<&encoding::Encoding>,
) -> Result<SimpleMapping> {
    if kind == FontKind::TrueType && route == Route::Raw {
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

pub(super) fn cid_map(
    document: &Document,
    metrics_font: &Dictionary,
    id: Option<ObjectId>,
) -> Result<Option<CidToGidMap>> {
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
                let id = metrics_font
                    .get(b"CIDToGIDMap")
                    .and_then(Value::as_reference)
                    .ok_or_else(|| Error::syntax("CIDToGIDMap stream must be indirect"))?;
                let bytes = document
                    .stream_data(document.object(id)?)
                    .map_err(|e| e.in_object(id))?;
                // Check the decoded length before copying the data.
                if bytes.len() > CidToGidMap::MAX_BYTES {
                    return Err(Error::syntax("invalid CIDToGIDMap length").in_object(id));
                }
                Some(CidToGidMap::new(bytes.into()).map_err(|e| e.in_object(id))?)
            }
        }
    } else {
        None
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pagura_core::Name;
    use pagura_document::OpenOptions;
    use pagura_testkit::fixture;

    #[test]
    fn symbolic_truetype_ignores_encoding_and_nonsymbolic_uses_names() -> Result<()> {
        let document = Document::open(
            fixture("text", "type3-differences.pdf"),
            &OpenOptions::default(),
        )?;
        let font: Dictionary = [(Name::new(b"Encoding"), Value::name(b"InvalidEncoding"))]
            .into_iter()
            .collect();
        let mapping = simple_mapping(&document, &font, FontKind::TrueType, Route::Raw, None)?;
        assert!(mapping.names.iter().all(Option::is_none));
        assert!(mapping.unicode.iter().all(Option::is_none));
        assert!(simple_mapping(&document, &font, FontKind::TrueType, Route::Named, None).is_err());
        let mapping = simple_mapping(
            &document,
            &Dictionary::new(),
            FontKind::TrueType,
            Route::Raw,
            None,
        )?;
        assert!(mapping.names.iter().all(Option::is_none));
        let named: Dictionary = [(Name::new(b"Encoding"), Value::name(b"WinAnsiEncoding"))]
            .into_iter()
            .collect();
        let mapping = simple_mapping(&document, &named, FontKind::TrueType, Route::Named, None)?;
        assert_eq!(mapping.unicode[65].as_deref(), Some("A"));
        Ok(())
    }
}
