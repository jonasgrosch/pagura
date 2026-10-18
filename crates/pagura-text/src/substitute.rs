//! Explicit non-embedded simple-font substitution (9.6.2.2; ADR 0009).
use crate::{
    FontKind, PdfFont, Provider, WritingMode,
    encoding::{self, SimpleMapping},
    features,
    font::{Mapping, Source, to_unicode},
    metrics::Metrics,
    objects::{number, optional, required},
};
use pagura_core::{Diagnostic, Dictionary, Error, ObjectId, Repair, Result, Value};
use pagura_document::Document;
use std::sync::Arc;

/// Diagnostic tag for an explicitly provided replacement outline.
pub const SUBSTITUTED: Repair = Repair::new("font.substituted");

pub(crate) fn load(
    document: &Document,
    font: &Dictionary,
    descriptor: Option<&Dictionary>,
    kind: FontKind,
    provider: &dyn Provider,
) -> Result<PdfFont> {
    if !matches!(kind, FontKind::Type1 | FontKind::TrueType) {
        return Err(unavailable());
    }
    let base = required(document, font, b"BaseFont")?
        .as_name()
        .ok_or_else(|| Error::syntax("invalid BaseFont"))?;
    let standard = standard_name(base.as_bytes());
    if let Some(descriptor) = descriptor
        && optional(document, descriptor, b"Flags")?
            .and_then(Value::as_integer)
            .is_some_and(|flags| flags & 4 != 0)
        && !matches!(standard, Some(b"Symbol" | b"ZapfDingbats"))
    {
        return Err(unavailable());
    }

    let selected = match standard {
        Some(name) => name,
        None => closest(
            document,
            descriptor.ok_or_else(unavailable)?,
            font.get(b"FontDescriptor").and_then(Value::as_reference),
        )?,
    };
    let data = provider.standard_font(selected)?.ok_or_else(unavailable)?;
    let names = encoding::load(
        document,
        font.get(b"Encoding"),
        data.metrics.encoding.clone(),
    )?;
    let metrics = if standard.is_some() && optional(document, font, b"Widths")?.is_none() {
        Metrics::standard(&data.metrics, &names)
    } else {
        Metrics::simple(document, font, descriptor, 0.001)?
    };
    let mapping = Mapping::Simple(Box::new(SimpleMapping::new(names, document.limits())?));
    let to_unicode = to_unicode(document, font, &mapping);
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
        let mut diagnostic = Diagnostic::new(SUBSTITUTED, substitution_message(selected));
        if let Some(id) = font.get(b"FontDescriptor").and_then(Value::as_reference) {
            diagnostic = diagnostic.in_object(id);
        }
        document.report([diagnostic]);
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

fn closest(
    document: &Document,
    descriptor: &Dictionary,
    id: Option<ObjectId>,
) -> Result<&'static [u8]> {
    let flags = required(document, descriptor, b"Flags")?
        .as_integer()
        .and_then(|flags| u32::try_from(flags).ok())
        .ok_or_else(|| Error::syntax("invalid font descriptor Flags"))?;
    if flags & 4 != 0 {
        return Err(unavailable());
    }
    if flags & 32 == 0 {
        return Err(Error::syntax("font must declare Symbolic or Nonsymbolic"));
    }
    let italic = flags & 64 != 0 || number(document, descriptor, b"ItalicAngle", 0.0)? != 0.0;
    let weight = weight(document, descriptor, id)?;
    let bold = flags & 262_144 != 0 || weight >= 600.0;
    let family = if flags & 1 != 0 {
        0
    } else if flags & 2 != 0 {
        1
    } else {
        2
    };
    Ok(canonical(family, bold, italic))
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
    let mut diagnostic = Diagnostic::new(
        crate::repairs::DESCRIPTOR_WEIGHT,
        "invalid optional FontWeight ignored; using normal weight (9.8.1, Table 120)",
    );
    diagnostic.object = id;
    crate::repairs::Report::new(document).diagnostic(diagnostic)?;
    Ok(400.0)
}

/// Recognizes only known family/style spellings. A subset tag is exactly six
/// uppercase ASCII letters followed by `+` (9.9.2); aliases are a host choice.
pub(crate) fn standard_name(name: &[u8]) -> Option<&'static [u8]> {
    let name = if name.get(6) == Some(&b'+') && name.get(..6)?.iter().all(u8::is_ascii_uppercase) {
        name.get(7..)?
    } else {
        name
    };
    if name == b"Symbol" {
        return Some(b"Symbol");
    }
    if name == b"ZapfDingbats" {
        return Some(b"ZapfDingbats");
    }
    // The fixed maximum avoids allocating proportionally to arbitrary PDF names.
    if name.len() > 80 {
        return None;
    }
    let normalized: Vec<_> = name
        .iter()
        .filter(|b| !matches!(b, b'-' | b',' | b' '))
        .map(u8::to_ascii_lowercase)
        .collect();
    let normalized = normalized.strip_suffix(b"mt").unwrap_or(&normalized);
    for (prefix, family) in [
        (b"timesnewromanps".as_slice(), 1),
        (b"timesnewroman", 1),
        (b"couriernewps", 0),
        (b"couriernew", 0),
        (b"helvetica", 2),
        (b"arial", 2),
        (b"courier", 0),
        (b"times", 1),
    ] {
        if let Some(style) = normalized.strip_prefix(prefix) {
            let (bold, italic) = match style {
                b"" | b"roman" | b"regular" => (false, false),
                // Short style names are a host alias policy (9.6.2.1).
                // Repeated b represents an already-bold face selected as bold.
                b"bold" | b"b" | b"bb" => (true, false),
                b"italic" | b"oblique" | b"i" => (false, true),
                b"bolditalic" | b"boldoblique" | b"bi" | b"ib" => (true, true),
                _ => continue,
            };
            return Some(canonical(family, bold, italic));
        }
    }
    None
}

fn canonical(family: u8, bold: bool, italic: bool) -> &'static [u8] {
    match (family, bold, italic) {
        (0, false, false) => b"Courier",
        (0, true, false) => b"Courier-Bold",
        (0, false, true) => b"Courier-Oblique",
        (0, true, true) => b"Courier-BoldOblique",
        (1, false, false) => b"Times-Roman",
        (1, true, false) => b"Times-Bold",
        (1, false, true) => b"Times-Italic",
        (1, true, true) => b"Times-BoldItalic",
        (_, false, false) => b"Helvetica",
        (_, true, false) => b"Helvetica-Bold",
        (_, false, true) => b"Helvetica-Oblique",
        (_, true, true) => b"Helvetica-BoldOblique",
    }
}

fn substitution_message(name: &[u8]) -> &'static str {
    match name {
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
