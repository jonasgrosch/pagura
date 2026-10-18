//! Read-only descriptor normalization (ISO 32000-2 9.8.2/Table 121).
//! Preserve Symbolic as the primary bit unless concrete encoding evidence
//! determines another usable route (9.6.5.2/4). Source objects stay immutable.
use pagura_core::{Diagnostic, Diagnostics, Dictionary, Error, Name, Position, Result, Value};
use pagura_document::Document;
use pagura_text::{FontKind, GlyphShape, PdfFont, Provider};
use std::borrow::Cow;

mod descriptor;
mod evidence;

pub(super) fn load(
    document: &Document,
    font: &Dictionary,
    provider: Option<&dyn Provider>,
    position: Position,
) -> Result<PdfFont> {
    let Some(descriptor) = descriptor::Descriptor::find(document, font)? else {
        return original(document, font, provider);
    };
    let Some(flags) = descriptor.flags(document)? else {
        return original(document, font, provider);
    };
    let symbolic = flags & 4 != 0;
    let invalid = symbolic == (flags & 32 != 0);
    let mut input = Cow::Borrowed(font);
    if invalid {
        report(document, &descriptor, position)?;
        input = Cow::Owned(descriptor.corrected(document, symbolic)?);
    }
    let loaded = original(document, &input, provider)?;
    if matches!(loaded.kind(), FontKind::Type0 | FontKind::Type3) {
        return Ok(loaded);
    }
    let glyphs = match loaded.decode(&[0]) {
        Ok(glyphs) => glyphs,
        Err(error)
            if error.kind
                == pagura_core::ErrorKind::Unsupported(pagura_text::features::NON_EMBEDDED) =>
        {
            return Ok(loaded);
        }
        Err(error) => return Err(error),
    };
    let Some(GlyphShape::Outline { program, .. }) = glyphs.first().map(|g| &g.shape) else {
        return Ok(loaded);
    };
    let latin = loaded.kind() == FontKind::TrueType && latin_encoding(document, font, symbolic)?;
    let Some(resolved) = evidence::symbolic(program, loaded.kind(), latin) else {
        return Ok(loaded);
    };
    if symbolic == resolved {
        return Ok(loaded);
    }
    if !invalid {
        report(document, &descriptor, position)?;
    }
    original(
        document,
        &descriptor.corrected(document, resolved)?,
        provider,
    )
}

fn original(
    document: &Document,
    font: &Dictionary,
    provider: Option<&dyn Provider>,
) -> Result<PdfFont> {
    provider.map_or_else(
        || PdfFont::load(document, font),
        |p| PdfFont::load_with(document, font, p),
    )
}

fn latin_encoding(document: &Document, font: &Dictionary, ignored: bool) -> Result<bool> {
    let Some(value) = font.get(b"Encoding") else {
        return Ok(false);
    };
    // On the Symbolic route Encoding is ignored (9.6.5.4). Only literal
    // metadata can provide contrary evidence; never follow its references.
    let resolve = |value| {
        if ignored {
            Ok(value)
        } else {
            document.resolve(value)
        }
    };
    let value = resolve(value)?;
    let name = if let Some(dict) = value.as_dictionary() {
        dict.get(b"BaseEncoding")
            .map(resolve)
            .transpose()?
            .and_then(Value::as_name)
    } else {
        value.as_name()
    };
    Ok(name.is_some_and(|n| matches!(n.as_bytes(), b"WinAnsiEncoding" | b"MacRomanEncoding")))
}

fn report(
    document: &Document,
    descriptor: &descriptor::Descriptor<'_>,
    position: Position,
) -> Result<()> {
    let mut diagnostics = Diagnostics::new(document.recovery());
    let mut diagnostic = Diagnostic::new(
        pagura_font::tags::DESCRIPTOR_FLAGS,
        "font descriptor flags resolved from Symbolic and encoding evidence (9.8.2, Table 121)",
    )
    .at(position);
    diagnostic.object = descriptor.object;
    diagnostics.repair(diagnostic)?;
    document.report(diagnostics.take());
    Ok(())
}

#[cfg(test)]
mod tests;
