//! Narrow reported descriptor spelling recovery, after typed unavailability.
use super::{FontKind, PdfFont, load};
use crate::{
    descriptor::{Descriptor, FontFlags},
    objects::optional,
    repairs,
    report::Report,
};
use pagura_core::{Diagnostic, Dictionary, Error, ErrorKind, Position, Recovery, Result, Value};
use pagura_document::Document;

/// Failed tentative reads keep their ordinary caches/diagnostics. Only success
/// reports this repair; no source object is rewritten or provider retried.
pub(super) fn recover(
    document: &Document,
    font: &Dictionary,
    kind: FontKind,
    position: Position,
    original: Error,
) -> Result<PdfFont> {
    if document.recovery() != Recovery::Repair
        || kind != FontKind::TrueType
        || font.contains_key(b"FontDescriptor")
    {
        return Err(original);
    }
    match attempt(document, font, position) {
        Ok(Some(font)) => Ok(font),
        Err(error) if matches!(error.kind, ErrorKind::Limit | ErrorKind::Cycle) => Err(error),
        Ok(None) | Err(_) => Err(original),
    }
}

fn attempt(document: &Document, font: &Dictionary, position: Position) -> Result<Option<PdfFont>> {
    let Some(id) = font.get(b"GontDescriptor").and_then(Value::as_reference) else {
        return Ok(None);
    };
    if !document.contains_object(id) {
        return Ok(None);
    }
    let object = document.object(id)?;
    let Some(dictionary) = object.dictionary().filter(|_| !object.is_stream()) else {
        return Ok(None);
    };
    let name = dictionary.get(b"FontName").and_then(Value::as_name);
    if !dictionary.is_name(b"Type", b"FontDescriptor")
        || name.is_none()
        || name != font.get(b"BaseFont").and_then(Value::as_name)
        || dictionary.contains_key(b"FontFile")
        || dictionary.contains_key(b"FontFile3")
    {
        return Ok(None);
    }
    let descriptor = Descriptor {
        dictionary,
        id: Some(id),
    };
    let Some(flags) = descriptor.flags(document)? else {
        return Ok(None);
    };
    if !flags.is_consistent()
        || !flags.contains(FontFlags::NONSYMBOLIC)
        || !source_tables(document, font)?
    {
        return Ok(None);
    }
    let Some(program_id) = dictionary.get(b"FontFile2").and_then(Value::as_reference) else {
        return Ok(None);
    };
    if !document.contains_object(program_id) || !document.object(program_id)?.is_stream() {
        return Ok(None);
    }
    let loaded = load::recovered_descriptor(document, font, descriptor)?;
    Report::new(document).diagnostic(
        Diagnostic::new(
            repairs::DESCRIPTOR_KEY,
            "GontDescriptor used as the matching FontDescriptor after unavailable font loading",
        )
        .in_object(id)
        .at(position),
    )?;
    Ok(Some(loaded))
}

/// The narrow alias proof neither guesses an encoding nor repairs widths.
/// A dictionary's undefined base slots remain undefined, not Standard fallbacks.
fn source_tables(document: &Document, font: &Dictionary) -> Result<bool> {
    let Some(encoding) = optional(document, font, b"Encoding")? else {
        return Ok(false);
    };
    let name = if let Some(dictionary) = encoding.as_dictionary() {
        optional(document, dictionary, b"BaseEncoding")?.and_then(Value::as_name)
    } else {
        encoding.as_name()
    };
    if !name.is_some_and(|name| {
        matches!(
            name.as_bytes(),
            b"WinAnsiEncoding" | b"MacRomanEncoding" | b"MacExpertEncoding"
        )
    }) {
        return Ok(false);
    }
    let code = |key: &[u8]| -> Result<Option<u8>> {
        Ok(optional(document, font, key)?
            .and_then(Value::as_integer)
            .and_then(|value| u8::try_from(value).ok()))
    };
    let (Some(first), Some(last)) = (code(b"FirstChar")?, code(b"LastChar")?) else {
        return Ok(false);
    };
    let Some(widths) = optional(document, font, b"Widths")?.and_then(Value::as_array) else {
        return Ok(false);
    };
    Ok(last >= first && widths.len() == usize::from(last - first) + 1)
}
