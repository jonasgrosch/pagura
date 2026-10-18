//! Repair invalid flag pairs, preserving the primary Symbolic bit (9.8.2).
//! Valid flags select the existing 9.6.5.2/4 routes, including the permitted
//! TrueType fallbacks. Repertoire metadata alone never changes that selection.
//! Source objects stay immutable.
use pagura_core::{Diagnostic, Diagnostics, Dictionary, Error, Name, Position, Result, Value};
use pagura_document::Document;
use pagura_text::{PdfFont, Provider};

mod descriptor;

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
    if symbolic != (flags & 32 != 0) {
        return original(document, font, provider);
    }
    report(document, &descriptor, position)?;
    original(
        document,
        &descriptor.corrected(document, symbolic)?,
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

fn report(
    document: &Document,
    descriptor: &descriptor::Descriptor<'_>,
    position: Position,
) -> Result<()> {
    let mut diagnostics = Diagnostics::new(document.recovery());
    let mut diagnostic = Diagnostic::new(
        pagura_font::tags::DESCRIPTOR_FLAGS,
        "font descriptor must set exactly one of Symbolic and Nonsymbolic (9.8.2, Table 121)",
    )
    .at(position);
    diagnostic.object = descriptor.object;
    diagnostics.repair(diagnostic)?;
    document.report(diagnostics.take());
    Ok(())
}

#[cfg(test)]
mod tests;
