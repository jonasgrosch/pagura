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

use pagura_core::{Dictionary, Value};
use pagura_document::{Document, Page};

/// Whether the document declares a PDF/X print intent: an output intent
/// with `S /GTS_PDFX` on the page or in the catalog, or `GTS_PDFXVersion` in
/// the Info dictionary (14.11.5). Advisory metadata never fails a page.
pub(crate) fn print_intent(document: &Document, page: &Page) -> bool {
    let info = document
        .trailer()
        .get(b"Info")
        .and_then(|v| document.resolve_dictionary(v).ok())
        .and_then(|d| d.get(b"GTS_PDFXVersion"))
        .and_then(|v| document.resolve(v).ok())
        .and_then(Value::as_string);
    info.is_some()
        || output_intent(document, page.dictionary())
            .or_else(|| {
                document
                    .catalog()
                    .ok()
                    .and_then(|d| output_intent(document, d))
            })
            .unwrap_or(false)
}

// A resolved array is authoritative even when empty or without a PDF/X
// entry (14.11.5). Only an absent or unusable array permits the catalog's.
fn output_intent(document: &Document, dictionary: &Dictionary) -> Option<bool> {
    let intents = dictionary
        .get(b"OutputIntents")
        .and_then(|v| document.resolve(v).ok())
        .and_then(Value::as_array)?;
    Some(intents.iter().take(document.limits().value_nodes).any(|v| {
        document
            .resolve_dictionary(v)
            .ok()
            .and_then(|d| d.get(b"S"))
            .and_then(|s| document.resolve(s).ok())
            .and_then(Value::as_name)
            .is_some_and(|s| s == b"GTS_PDFX")
    }))
}
