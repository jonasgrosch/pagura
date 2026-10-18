//! Collection-to-Unicode substitution policy (9.7.4.2–9.7.5; ADR 0009).
use crate::{
    CidToUnicode, FontKind, PdfFont, Provider, WritingMode, features,
    font::{Mapping, Source, cid_collection, to_unicode},
    metrics::Metrics,
};
use pagura_core::{Diagnostic, Dictionary, Error, ObjectId, Repair, Result};
use pagura_document::Document;
use pagura_font::{CmapSubtable, FontProgram, GlyphId};

/// A substitute cannot draw every CID reachable through the encoding CMap.
/// Reported once at font load, including notdef coverage, rather than retaining
/// a document reference or reporting repeatedly during extraction. Missing
/// values draw no mark and retain their original PDF metrics in both modes.
/// This describes a provider coverage limitation, not malformed PDF input.
pub const CID_GLYPH_MISSING: Repair = Repair::new("font.cid-glyph-missing");

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
    let (registry, ordering) = cid_collection(document, descendant)?.ok_or_else(unavailable)?;
    if map
        .registry
        .as_ref()
        .is_some_and(|r| r.as_bytes() != registry)
        || map
            .ordering
            .as_ref()
            .is_some_and(|o| o.as_bytes() != ordering)
    {
        return Err(Error::syntax(
            "CIDFont and CMap character collections disagree (9.7.3)",
        ));
    }
    let program = provider
        .cid_font(registry, ordering)?
        .ok_or_else(unavailable)?;
    let unicode = provider
        .cid_to_unicode(registry, ordering)?
        .ok_or_else(unavailable)?;
    let metrics = Metrics::composite(document, descendant)?;
    // Inspect reachable CIDs, not the substitute's own CID charset. Stop at the
    // first hole; the diagnostic covers incomplete font coverage as a whole.
    let mut work = 0;
    for cid in map.cids() {
        crate::cmap::charge(&mut work, 1, document.limits().operations)?;
        if glyph(&program, Some(&unicode), cid).is_none() {
            report(
                document,
                id,
                CID_GLYPH_MISSING,
                "CID substitute has incomplete Unicode/glyph coverage; missing CIDs paint nothing and retain PDF advances",
            );
            break;
        }
    }
    let to_unicode = to_unicode(document, font, &mapping);
    report(
        document,
        id,
        crate::SUBSTITUTED,
        "non-embedded CID font uses a collection-specific Unicode substitute (9.7.4.2); PDF widths retained",
    );
    Ok(PdfFont {
        kind: FontKind::Type0,
        writing_mode: WritingMode::Horizontal,
        source: Source::CidSubstitute { program },
        mapping,
        metrics,
        to_unicode,
        limits: document.limits(),
        cid_unicode: Some(unicode),
        retained: 0,
    }
    .accounted())
}

pub(crate) fn glyph(
    program: &FontProgram,
    unicode: Option<&CidToUnicode>,
    cid: u32,
) -> Option<GlyphId> {
    let mut chars = unicode?.get(cid)?.chars();
    let scalar = chars.next()?;
    // Collection aliases have already been projected by the provider. A custom
    // provider's multi-scalar sequence cannot identify one glyph without shaping.
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

fn report(document: &Document, id: Option<ObjectId>, tag: Repair, message: &'static str) {
    let mut diagnostic = Diagnostic::new(tag, message);
    diagnostic.object = id;
    document.report([diagnostic]);
}

fn unavailable() -> Error {
    Error::unsupported(
        features::NON_EMBEDDED,
        "CID font has no available collection-specific substitute",
    )
}
