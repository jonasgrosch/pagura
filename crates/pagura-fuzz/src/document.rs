//! `document`: opening, lazy object loading, stream decoding, page
//! enumeration and content compilation return structured errors in both
//! recovery modes, and strict mode never repairs (ADR 0003).

use pagura::{Document, OpenOptions, Recovery};

/// How many objects of one document are loaded and decoded.
const OBJECTS: usize = 256;
/// How many pages of one document are compiled.
const PAGES: usize = 4;

/// Opens the input as a PDF file and walks it.
///
/// # Panics
///
/// When the input violates a checked property: that is a finding.
pub fn run(data: &[u8]) {
    for recovery in [Recovery::Strict, Recovery::Repair] {
        let options = OpenOptions {
            limits: super::limits(),
            recovery,
            ..OpenOptions::default()
        };
        let Some(document) = super::checked(Document::open(data.to_vec(), &options)) else {
            continue;
        };
        super::assert_diagnostics(&document.diagnostics(), recovery);
        super::checked(document.catalog());
        let ids: Vec<_> = document.object_ids().take(OBJECTS).collect();
        for id in ids {
            if let Some(object) = super::checked(document.object(id))
                && object.is_stream()
            {
                super::checked(document.stream_data(object));
            }
        }
        if let Some(pages) = super::checked(document.pages()) {
            for page in pages.iter().take(PAGES) {
                super::checked(document.content_streams(page));
                super::checked(pagura::compile_page(&document, page));
            }
        }
        super::assert_diagnostics(&document.diagnostics(), recovery);
    }
}
