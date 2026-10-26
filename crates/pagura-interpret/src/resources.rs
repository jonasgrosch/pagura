//! Readers of the document objects content refers to: colour spaces,
//! functions, shadings, patterns, transparency groups and the
//! optional-content configuration. They know nothing about devices.

pub(crate) mod colorspace;
pub(crate) mod function;
pub(crate) mod group;
pub(crate) mod numbers;
pub(crate) mod optional;
pub(crate) mod pattern;
pub(crate) mod shading;

use pagura_core::{Dictionary, ErrorKind, Value};
use pagura_document::source::{ReadFault, ReadResult};
use pagura_document::{Document, Page};

/// An unusable advisory value is absent. Availability, source lifecycle and
/// resource-budget failures retain their meaning even for advisory metadata.
/// This is shared by print-intent and font-bound readers.
pub(crate) fn advisory<T>(result: ReadResult<T>) -> ReadResult<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(ReadFault::Invalid(error))
            if matches!(
                error.kind,
                ErrorKind::Syntax | ErrorKind::MissingObject | ErrorKind::Cycle
            ) =>
        {
            Ok(None)
        }
        Err(fault) => Err(fault),
    }
}

/// Whether the document declares a PDF/X print intent: an output intent
/// with `S /GTS_PDFX` on the page or in the catalog, or `GTS_PDFXVersion` in
/// the Info dictionary (14.11.5). Malformed advisory metadata is ignored;
/// unavailable dependencies wait before a print policy is selected.
pub(crate) fn print_intent(document: &Document, page: &Page) -> ReadResult<bool> {
    document.check_page(page)?;
    if let Some(info) = document.trailer().get(b"Info")
        && let Some(dictionary) = advisory(document.resolve_dictionary(info))?
        && advisory(document.optional(dictionary, b"GTS_PDFXVersion"))?
            .flatten()
            .and_then(Value::as_string)
            .is_some()
    {
        return Ok(true);
    }
    if let Some(intent) = output_intent(document, page.dictionary())? {
        return Ok(intent);
    }
    let Some(catalog) = advisory(document.catalog())? else {
        return Ok(false);
    };
    Ok(output_intent(document, catalog)?.unwrap_or(false))
}

// A resolved array is authoritative even when empty or without a PDF/X
// entry (14.11.5). Only an absent or unusable array permits the catalog's.
fn output_intent(document: &Document, dictionary: &Dictionary) -> ReadResult<Option<bool>> {
    let Some(intents) = advisory(document.optional(dictionary, b"OutputIntents"))?
        .flatten()
        .and_then(Value::as_array)
    else {
        return Ok(None);
    };
    for value in intents.iter().take(document.limits().value_nodes) {
        if let Some(dictionary) = advisory(document.resolve_dictionary(value))?
            && advisory(document.optional(dictionary, b"S"))?
                .flatten()
                .and_then(Value::as_name)
                .is_some_and(|name| name == b"GTS_PDFX")
        {
            return Ok(Some(true));
        }
    }
    Ok(Some(false))
}
