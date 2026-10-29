//! Collection-to-Unicode substitution policy (9.7.4.2–9.7.5; ADR 0009).
use crate::{
    CidToUnicode, FontKind, PdfFont, Provider, WritingMode,
    cid::Cid,
    features,
    font::{Mapping, Source, cid_collection},
    metrics::Metrics,
    report::Report,
};
use pagura_core::{Diagnostic, Dictionary, Error, Meter, ObjectId, Result};
use pagura_document::Document;
use pagura_font::{CmapSubtable, FontProgram, GlyphId};

pub(crate) fn load(
    document: &Document,
    descendant: &Dictionary,
    id: Option<ObjectId>,
    mapping: Mapping,
    provider: &dyn Provider,
) -> Result<PdfFont> {
    let Mapping::Composite { map, .. } = &mapping else {
        return Err(unavailable());
    };
    let (registry, ordering) = cid_collection(document, descendant)?.ok_or_else(unavailable)?;
    if map.registry().is_some_and(|r| r != registry)
        || map.ordering().is_some_and(|o| o != ordering)
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
    let writing_mode = map.mode;
    let vertical = if writing_mode == WritingMode::Vertical {
        // Regional collection policy chooses the font's Han language system.
        // This changes glyph forms only: PDF W/W2/DW/DW2 still own positioning.
        let language =
            (registry == b"Adobe")
                .then_some(ordering)
                .and_then(|ordering| match ordering {
                    b"Japan1" => Some(*b"JAN "),
                    b"GB1" => Some(*b"ZHS "),
                    b"CNS1" => Some(*b"ZHT "),
                    b"Korea1" => Some(*b"KOR "),
                    _ => None,
                });
        Some(
            program
                .vertical_substitution(*b"hani", language)?
                .ok_or_else(|| {
                    Error::unsupported(
                        features::CID_SUBSTITUTE_VERTICAL,
                        "CID substitute lacks an applicable vertical glyph feature",
                    )
                })?,
        )
    } else {
        None
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
        writing_mode,
        source: Source::CidSubstitute {
            program,
            unicode,
            vertical,
        },
        mapping,
        metrics,
        limits: document.limits(),
        retained: 0,
    }
    .accounted())
}

pub(crate) fn glyph(
    program: &FontProgram,
    unicode: Option<&CidToUnicode>,
    cid: Cid,
) -> Option<GlyphId> {
    let mut chars = unicode?.get(u32::from(cid))?.chars();
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

fn unavailable() -> Error {
    Error::unsupported(
        features::NON_EMBEDDED,
        "CID font has no available collection-specific substitute",
    )
}
