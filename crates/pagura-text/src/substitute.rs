//! Explicit non-embedded simple-font substitution (9.6.2.2; ADR 0009).
use crate::{
    FontKind, PdfFont, Provider, WritingMode,
    descriptor::{Descriptor, FontFlags},
    encoding::{self, SimpleMapping},
    features,
    font::{Mapping, Ownership, Source},
    metrics::{GLYPH_SPACE_UNITS, Metrics},
    objects::{number, optional, required},
    report::Report,
    selection::Route,
};
use pagura_core::{Diagnostic, Dictionary, Error, Position, Result};
use pagura_document::Document;
use std::sync::Arc;

pub(crate) mod cid;
mod family;
use family::{Family, Style};
pub(crate) use family::{Standard14, standard_name};

/// The `/FontWeight` values Table 120 allows.
const FONT_WEIGHTS: [f64; 9] = [
    100.0, 200.0, 300.0, 400.0, 500.0, 600.0, 700.0, 800.0, 900.0,
];
/// The normal weight, used when `/FontWeight` is absent or invalid.
const NORMAL_WEIGHT: f64 = 400.0;
/// The lightest weight substitution treats as bold.
const BOLD_WEIGHT: f64 = 600.0;

pub(crate) fn load(
    document: &Document,
    font: &Dictionary,
    descriptor: Option<Descriptor<'_>>,
    kind: FontKind,
    provider: &dyn Provider,
    position: Position,
) -> Result<PdfFont> {
    if !matches!(kind, FontKind::Type1 | FontKind::TrueType) {
        return Err(unavailable());
    }
    let base = required(document, font, b"BaseFont")?
        .as_name()
        .ok_or_else(|| Error::syntax("invalid BaseFont"))?;
    let standard = standard_name(base.as_bytes());
    let flags = descriptor
        .map(|descriptor| descriptor.normalized_flags(document, position))
        .transpose()?;
    let symbolic = flags.is_some_and(|flags| flags.contains(FontFlags::SYMBOLIC));
    if symbolic && !standard.is_some_and(Standard14::is_symbolic) {
        return Err(unavailable());
    }
    let selected = match standard {
        Some(face) => face,
        None => match (descriptor, flags) {
            (Some(descriptor), Some(flags)) => closest(document, descriptor, flags)?,
            _ => return Err(unavailable()),
        },
    };
    let data = provider
        .standard_font(selected.name().as_bytes())?
        .ok_or_else(unavailable)?;
    let encoding = if kind == FontKind::TrueType && symbolic {
        None
    } else {
        font.get(b"Encoding")
    };
    let mut names = encoding::load(document, encoding, data.metrics.encoding.clone())?;
    // A named Latin encoding can accompany a nonsymbolic declaration on a
    // known Symbol TrueType face. After the name route fails, 9.6.5.4 permits
    // reader fallbacks: use the known AFM code mapping. Explicit Differences
    // retain their names, including undefined glyphs; no name is guessed.
    if kind == FontKind::TrueType
        && standard.is_some_and(Standard14::is_symbolic)
        && optional(document, font, b"Encoding")?.is_some_and(|v| v.as_name().is_some())
        && let Some(program) = &data.program
    {
        for (name, builtin) in names.iter_mut().zip(&data.metrics.encoding) {
            if name
                .as_ref()
                .is_none_or(|n| program.glyph_for_name(n.as_bytes()).is_none())
            {
                name.clone_from(builtin);
            }
        }
    }
    let metrics = if standard.is_some() && optional(document, font, b"Widths")?.is_none() {
        Metrics::standard(&data.metrics, &names)
    } else {
        Metrics::simple(
            document,
            font,
            descriptor.map(|d| d.dictionary),
            GLYPH_SPACE_UNITS,
        )?
    };
    let mapping = Mapping::Simple(Box::new(SimpleMapping::new(
        names,
        &document.limits(),
        base.as_bytes(),
    )?));
    let source = data
        .program
        .as_ref()
        .map_or(Source::MetricsOnly, |program| Source::Program {
            program: Arc::clone(program),
            route: Route::Named,
            ownership: Ownership::Provider,
            cid_map: None,
        });
    if data.program.is_some() {
        // Substitution is permitted PDF behavior, not a repair of malformed input.
        // Report it in strict mode too; no recovery permission is consumed.
        Report::new(document).diagnostic(
            Diagnostic::information(
                crate::information::SUBSTITUTED,
                selected.substitution_message(),
            )
            .in_object_opt(descriptor.and_then(|d| d.id)),
        )?;
    }
    Ok(PdfFont {
        kind,
        writing_mode: WritingMode::Horizontal,
        source,
        mapping,
        metrics,
        limits: document.limits(),
        retained: 0,
    }
    .accounted())
}

fn unavailable() -> Error {
    Error::unsupported(
        features::NON_EMBEDDED,
        "font has no safe available substitute",
    )
}

/// The Latin face closest to a nonsymbolic descriptor (9.8.1–9.8.2); `flags`
/// are normalized, so exactly one of Symbolic and Nonsymbolic is set.
fn closest(
    document: &Document,
    descriptor: Descriptor<'_>,
    flags: FontFlags,
) -> Result<Standard14> {
    if flags.contains(FontFlags::SYMBOLIC) {
        return Err(unavailable());
    }
    let italic = flags.contains(FontFlags::ITALIC)
        || number(document, descriptor.dictionary, b"ItalicAngle", 0.0)? != 0.0;
    // An invalid FontWeight is reported even when ForceBold decides boldness.
    let weight = weight(document, descriptor)?;
    let bold = flags.contains(FontFlags::FORCE_BOLD) || weight >= BOLD_WEIGHT;
    let style = match (bold, italic) {
        (false, false) => Style::Regular,
        (true, false) => Style::Bold,
        (false, true) => Style::Italic,
        (true, true) => Style::BoldItalic,
    };
    Ok(Family::from_flags(flags).face(style))
}

fn weight(document: &Document, descriptor: Descriptor<'_>) -> Result<f64> {
    let Some(value) = optional(document, descriptor.dictionary, b"FontWeight")? else {
        return Ok(NORMAL_WEIGHT);
    };
    if let Some(weight) = value.as_number()
        && FONT_WEIGHTS.contains(&weight)
    {
        return Ok(weight);
    }
    Report::new(document).diagnostic(
        Diagnostic::new(
            crate::repairs::DESCRIPTOR_WEIGHT,
            "invalid optional FontWeight ignored; using normal weight (9.8.1, Table 120)",
        )
        .in_object_opt(descriptor.id),
    )?;
    Ok(NORMAL_WEIGHT)
}
