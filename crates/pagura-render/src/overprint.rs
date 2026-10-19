//! Overprint preview selection and PDF/X declarations (14.11.5; ISO 15930).

use pagura_core::{Dictionary, Value};
use pagura_document::{Document, Page};

/// Display policy for overprinting (8.6.7; 11.7.4), independent of compilation.
/// Disabling simulation paints overprinting marks as ordinary composite content.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum OverprintPreview {
    /// Simulate only when PDF/X print intent is declared: an output intent with
    /// `S /GTS_PDFX`, or a `GTS_PDFXVersion` string in the document's Info dictionary.
    /// This detects a declaration, not PDF/X conformance or an ICC output transform.
    #[default]
    Auto,
    /// Simulate wherever overprinting can affect painted colourants.
    Always,
    /// Ignore overprint flags and paint ordinary composite content.
    Never,
}

/// Document policy resolved before painting; independent of page colourants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Policy {
    Ignore,
    Simulate,
}

impl OverprintPreview {
    pub(crate) fn resolve(self, print_intent: bool) -> Policy {
        match self {
            Self::Always => Policy::Simulate,
            Self::Auto if print_intent => Policy::Simulate,
            Self::Auto | Self::Never => Policy::Ignore,
        }
    }
}

/// Advisory metadata must not turn a paintable page into a rendering error,
/// so every unreadable entry counts as absent. Core reports a reference cycle
/// as `Limit`, which leaves no budget error to tell apart here.
/// Page-level intents are also permitted by PDF 2.0 (14.11.5, Table 401).
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

// A resolved array is authoritative even when empty or without a PDF/X entry
// (14.11.5). Only an absent or unusable array permits catalog fallback.
fn output_intent(document: &Document, dictionary: &Dictionary) -> Option<bool> {
    let intents = dictionary
        .get(b"OutputIntents")
        .and_then(|v| document.resolve(v).ok())
        .and_then(Value::as_array)?;
    Some(intents.iter().any(|v| {
        document
            .resolve_dictionary(v)
            .ok()
            .and_then(|d| d.get(b"S"))
            .and_then(|s| document.resolve(s).ok())
            .and_then(Value::as_name)
            .is_some_and(|s| s == b"GTS_PDFX")
    }))
}
