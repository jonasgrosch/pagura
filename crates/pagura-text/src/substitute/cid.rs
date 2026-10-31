//! Collection-to-Unicode substitution policy (9.7.4.2–9.7.5; ADR 0009).
use super::{Standard14, closest, standard_name};
use crate::objects::optional;
use crate::{
    CidToUnicode, FontKind, PdfFont, Provider, WritingMode,
    cid::Cid,
    descriptor::{Descriptor, FontFlags},
    features,
    font::{Mapping, Source, cid_collection},
    metrics::Metrics,
    repairs,
    report::Report,
};
use pagura_core::{Diagnostic, Dictionary, Error, Meter, ObjectId, Recovery, Result, Value};
use pagura_document::Document;
use pagura_font::{CmapSubtable, FontProgram, GlyphId};
use std::sync::Arc;

pub(crate) fn load(
    document: &Document,
    font: &Dictionary,
    descendant: &Dictionary,
    id: Option<ObjectId>,
    mapping: Mapping,
    provider: &dyn Provider,
) -> Result<PdfFont> {
    let Mapping::Composite { map, .. } = &mapping else {
        return Err(unavailable());
    };
    if map.mode == WritingMode::Vertical {
        return Err(Error::unsupported(
            features::CID_SUBSTITUTE_VERTICAL,
            "vertical CID substitute glyph forms are unavailable",
        ));
    }
    let collection = cid_collection(document, descendant)?;
    if let Some((registry, ordering)) = collection {
        if map.registry().is_some_and(|r| r != registry)
            || map.ordering().is_some_and(|o| o != ordering)
        {
            return Err(Error::syntax(
                "CIDFont and CMap character collections disagree (9.7.3)",
            ));
        }
        if let Some(program) = provider.cid_font(registry, ordering)? {
            let unicode = provider
                .cid_to_unicode(registry, ordering)?
                .ok_or_else(unavailable)?;
            return collection_substitute(document, descendant, id, mapping, program, unicode);
        }
    }
    // An Identity or unnamed collection shares no glyph identity with any
    // substitute. A known Latin family draws through the font's own ToUnicode
    // text with the metric-compatible bundled face; another name with a
    // nonsymbolic descriptor and ToUnicode text, with the descriptor's
    // closest face as a reported repair (ADR 0009).
    if collection.is_none_or(|(_, ordering)| ordering == b"Identity") {
        if let Some(face) = latin_face(document, descendant)? {
            return latin(document, font, descendant, id, mapping, face, provider);
        }
        if let Some((descriptor, flags)) = nonsymbolic(document, font, descendant)? {
            if document.recovery() == Recovery::Strict {
                return Err(unavailable().with_repair(repairs::CID_DESCRIPTOR_FACE));
            }
            let face = closest(document, descriptor, flags)?;
            Report::new(document).diagnostic(
                Diagnostic::new(
                    repairs::CID_DESCRIPTOR_FACE,
                    "Identity CIDFont of an unknown family drawn with the Latin face closest to its descriptor (9.8.1)",
                )
                .in_object_opt(id),
            )?;
            return latin(document, font, descendant, id, mapping, face, provider);
        }
    }
    Err(unavailable())
}

/// The collection route: CIDs select the collection's Unicode, which selects
/// the substitute's glyph (9.7.4.2).
fn collection_substitute(
    document: &Document,
    descendant: &Dictionary,
    id: Option<ObjectId>,
    mapping: Mapping,
    program: Arc<FontProgram>,
    unicode: Arc<CidToUnicode>,
) -> Result<PdfFont> {
    let Mapping::Composite { map, .. } = &mapping else {
        return Err(unavailable());
    };
    let metrics = Metrics::composite(document, descendant)?;
    // Inspect reachable CIDs, not the substitute's own CID charset. Stop at the
    // first hole; the diagnostic covers incomplete font coverage as a whole.
    let report = Report::new(document);
    let mut work = Meter::new(
        document.limits().font_work,
        "CID substitute coverage work budget exceeded",
    );
    for cid in map.cids() {
        work.charge(1)?;
        if glyph(&program, Some(&unicode), cid).is_none() {
            report.diagnostic(
                Diagnostic::information(
                    crate::information::CID_GLYPH_MISSING,
                    "CID substitute has incomplete Unicode/glyph coverage; missing CIDs paint nothing and retain PDF advances",
                )
                .in_object_opt(id),
            )?;
            break;
        }
    }
    report.diagnostic(
        Diagnostic::information(
            crate::information::SUBSTITUTED,
            "non-embedded CID font uses a collection-specific Unicode substitute (9.7.4.2); PDF widths retained",
        )
        .in_object_opt(id),
    )?;
    Ok(PdfFont {
        kind: FontKind::Type0,
        writing_mode: WritingMode::Horizontal,
        source: Source::CidSubstitute { program, unicode },
        mapping,
        metrics,
        limits: document.limits(),
        retained: 0,
    }
    .accounted())
}

/// The Latin route: a known Latin family's bundled face, whose glyphs each
/// code's ToUnicode text selects. The PDF's `W` and `DW` keep the advances.
fn latin(
    document: &Document,
    font: &Dictionary,
    descendant: &Dictionary,
    id: Option<ObjectId>,
    mapping: Mapping,
    face: Standard14,
    provider: &dyn Provider,
) -> Result<PdfFont> {
    let program = provider
        .standard_font(face.name().as_bytes())?
        .and_then(|data| data.program.clone())
        .ok_or_else(unavailable)?;
    let to_unicode = crate::font::to_unicode(document, font, &mapping)?;
    let metrics = Metrics::composite(document, descendant)?;
    let report = Report::new(document);
    // Codes without ToUnicode text, notdef among them, select no glyph.
    report.diagnostic(
        Diagnostic::information(
            crate::information::CID_GLYPH_MISSING,
            "Identity CID substitute draws only codes with ToUnicode text; others paint nothing and retain PDF advances",
        )
        .in_object_opt(id),
    )?;
    report.diagnostic(
        Diagnostic::information(
            crate::information::SUBSTITUTED,
            "non-embedded Identity CID font uses a Latin substitute through its ToUnicode map; PDF widths retained",
        )
        .in_object_opt(id),
    )?;
    Ok(PdfFont {
        kind: FontKind::Type0,
        writing_mode: WritingMode::Horizontal,
        source: Source::UnicodeSubstitute {
            program,
            to_unicode,
        },
        mapping,
        metrics,
        limits: document.limits(),
        retained: 0,
    }
    .accounted())
}

/// The CIDFont's descriptor and flags when they allow the closest Latin face
/// an unknown nonsymbolic simple font gets (9.8.1–9.8.2): `/Flags` set
/// Nonsymbolic consistently, and the font has a `ToUnicode` map to draw by.
fn nonsymbolic<'a>(
    document: &'a Document,
    font: &Dictionary,
    descendant: &'a Dictionary,
) -> Result<Option<(Descriptor<'a>, FontFlags)>> {
    if optional(document, font, b"ToUnicode")?.is_none() {
        return Ok(None);
    }
    let Some(descriptor) = Descriptor::of(document, descendant)? else {
        return Ok(None);
    };
    Ok(descriptor
        .flags(document)?
        .filter(|flags| flags.is_consistent() && !flags.contains(FontFlags::SYMBOLIC))
        .map(|flags| (descriptor, flags)))
}

/// The Latin standard face a CIDFont's `/BaseFont` names, if any.
fn latin_face(document: &Document, descendant: &Dictionary) -> Result<Option<Standard14>> {
    Ok(optional(document, descendant, b"BaseFont")?
        .and_then(Value::as_name)
        .and_then(|name| standard_name(name.as_bytes()))
        .filter(|face| !face.is_symbolic()))
}

pub(crate) fn glyph(
    program: &FontProgram,
    unicode: Option<&CidToUnicode>,
    cid: Cid,
) -> Option<GlyphId> {
    unicode_glyph(program, unicode?.get(u32::from(cid))?)
}

/// The glyph of a single Unicode scalar in `program`'s Unicode cmap.
pub(crate) fn unicode_glyph(program: &FontProgram, text: &str) -> Option<GlyphId> {
    let mut chars = text.chars();
    let scalar = chars.next()?;
    // Collection aliases have already been projected by the provider. A custom
    // provider's multi-scalar sequence, or a ToUnicode ligature, cannot
    // identify one glyph without shaping.
    if chars.next().is_some() {
        return None;
    }
    [
        CmapSubtable::WindowsUnicodeFull,
        CmapSubtable::WindowsUnicode,
        CmapSubtable::Unicode,
    ]
    .into_iter()
    .find_map(|table| {
        program
            .glyph_for_code(table, u32::from(scalar))
            .filter(|gid| gid.0 != 0)
    })
}

fn unavailable() -> Error {
    Error::unsupported(
        features::NON_EMBEDDED,
        "CID font has no available collection-specific substitute",
    )
}
