//! Explicit non-embedded simple-font substitution (9.6.2.2; ADR 0009).
use crate::{
    FontKind, PdfFont, Provider, WritingMode,
    descriptor::{Descriptor, FontFlags},
    encoding::{self, SimpleMapping},
    features,
    font::{Mapping, Source, to_unicode},
    metrics::Metrics,
    objects::{number, optional, required},
    report::Report,
};
use pagura_core::{Diagnostic, Dictionary, Error, ObjectId, Position, Result, Value};
use pagura_document::Document;
use std::sync::Arc;

mod family;
use family::Family;
pub(crate) use family::standard_name;

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
    if symbolic && !matches!(standard, Some(b"Symbol" | b"ZapfDingbats")) {
        return Err(unavailable());
    }

    let selected = match standard {
        Some(name) => name,
        None => match (descriptor, flags) {
            (Some(descriptor), Some(flags)) => closest(document, descriptor, flags)?,
            _ => return Err(unavailable()),
        },
    };
    let data = provider.standard_font(selected)?.ok_or_else(unavailable)?;
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
        && matches!(standard, Some(b"Symbol" | b"ZapfDingbats"))
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
        Metrics::simple(document, font, descriptor.map(|d| d.dictionary), 0.001)?
    };
    let mapping = Mapping::Simple(Box::new(SimpleMapping::new(
        names,
        document.limits(),
        base.as_bytes(),
    )?));
    let to_unicode = to_unicode(document, font, &mapping)?;
    let source = data
        .program
        .as_ref()
        .map_or(Source::MetricsOnly, |program| Source::Program {
            program: Arc::clone(program),
            truetype_raw: false,
            substituted: true,
            cid_map: None,
        });
    if data.program.is_some() {
        // Substitution is permitted PDF behavior, not a repair of malformed input.
        // Report it in strict mode too; no recovery permission is consumed.
        Report::new(document).diagnostic(
            Diagnostic::information(
                crate::information::SUBSTITUTED,
                substitution_message(selected),
            )
            .in_object_opt(font.get(b"FontDescriptor").and_then(Value::as_reference)),
        )?;
    }
    Ok(PdfFont {
        kind,
        writing_mode: WritingMode::Horizontal,
        source,
        mapping,
        metrics,
        to_unicode,
        limits: document.limits(),
        cid_unicode: None,
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

/// The Latin face closest to a nonsymbolic descriptor; `flags` are already
/// normalized, so exactly one of Symbolic and Nonsymbolic is set.
fn closest(
    document: &Document,
    descriptor: Descriptor<'_>,
    flags: FontFlags,
) -> Result<&'static [u8]> {
    if flags.contains(FontFlags::SYMBOLIC) {
        return Err(unavailable());
    }
    let flags = flags.bits();
    let italic =
        flags & 64 != 0 || number(document, descriptor.dictionary, b"ItalicAngle", 0.0)? != 0.0;
    let weight = weight(document, descriptor.dictionary, descriptor.id)?;
    let bold = flags & 262_144 != 0 || weight >= 600.0;
    Ok(Family::from_flags(flags).canonical(bold, italic))
}

fn weight(document: &Document, descriptor: &Dictionary, id: Option<ObjectId>) -> Result<f64> {
    let Some(value) = optional(document, descriptor, b"FontWeight")? else {
        return Ok(400.0);
    };
    if let Some(weight) = value.as_number()
        && [
            100.0, 200.0, 300.0, 400.0, 500.0, 600.0, 700.0, 800.0, 900.0,
        ]
        .contains(&weight)
    {
        return Ok(weight);
    }
    Report::new(document).diagnostic(
        Diagnostic::new(
            crate::repairs::DESCRIPTOR_WEIGHT,
            "invalid optional FontWeight ignored; using normal weight (9.8.1, Table 120)",
        )
        .in_object_opt(id),
    )?;
    Ok(400.0)
}

fn substitution_message(name: &[u8]) -> &'static str {
    match name {
        b"Symbol" => "substitute outlines for Symbol",
        b"ZapfDingbats" => "substitute outlines for ZapfDingbats",
        b"Courier" => "substitute outlines for Courier",
        b"Courier-Bold" => "substitute outlines for Courier-Bold",
        b"Courier-Oblique" => "substitute outlines for Courier-Oblique",
        b"Courier-BoldOblique" => "substitute outlines for Courier-BoldOblique",
        b"Times-Roman" => "substitute outlines for Times-Roman",
        b"Times-Bold" => "substitute outlines for Times-Bold",
        b"Times-Italic" => "substitute outlines for Times-Italic",
        b"Times-BoldItalic" => "substitute outlines for Times-BoldItalic",
        b"Helvetica-Bold" => "substitute outlines for Helvetica-Bold",
        b"Helvetica-Oblique" => "substitute outlines for Helvetica-Oblique",
        b"Helvetica-BoldOblique" => "substitute outlines for Helvetica-BoldOblique",
        _ => "substitute outlines for Helvetica",
    }
}
