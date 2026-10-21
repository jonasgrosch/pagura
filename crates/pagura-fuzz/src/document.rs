//! `document`: opening, lazy object loading, stream decoding, page
//! enumeration and content compilation return structured errors in both
//! recovery modes, and strict mode never repairs (ADR 0003).

use pagura::{Document, ErrorKind, OpenOptions, Recovery};
use std::sync::Arc;

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
        if let Some(document) = super::checked(Document::open(data.to_vec(), &options)) {
            walk(document);
        }
    }
}

/// Loads and decodes the first objects and compiles the first pages of an
/// opened document through one render context, checking its diagnostics
/// before and after, and checks its signatures. Compiling a page again from the context's caches gives
/// a display list of the same length, unless a budget runs out.
pub(crate) fn walk(document: Document) {
    super::assert_diagnostics(&document.diagnostics(), document.recovery());
    super::checked(document.catalog());
    let ids: Vec<_> = document.object_ids().take(OBJECTS).collect();
    for id in ids {
        if let Some(object) = super::checked(document.object(id))
            && object.is_stream()
        {
            super::checked(document.stream_data(object));
        }
    }
    signatures(&document);
    let context = pagura::render_context(Arc::new(document));
    let document = context.document();
    if let Some(pages) = super::checked(document.pages()) {
        for page in pages.iter().take(PAGES) {
            super::checked(document.content_streams(page));
            if let Some(list) = super::checked(context.compile_page(page)) {
                match context.compile_page(page) {
                    Ok(again) => assert_eq!(again.len(), list.len(), "the recompilation differs"),
                    Err(error) => {
                        assert_eq!(
                            error.kind,
                            ErrorKind::Limit,
                            "the recompilation failed: {error}"
                        );
                    }
                }
            }
        }
    }
    super::assert_diagnostics(&document.diagnostics(), document.recovery());
}

/// Signature views and CMS integrity checks: every failure, per field or
/// per signature, is a structured error, and their diagnostics follow the
/// recovery mode.
fn signatures(document: &Document) {
    let Some(views) = super::checked(pagura::signatures(document)) else {
        return;
    };
    let signatures = views
        .fields
        .iter()
        .filter_map(|field| field.signature.as_ref())
        .chain(&views.certification)
        .chain(&views.usage_rights);
    for signature in signatures {
        let results = &signature.results;
        results
            .error
            .iter()
            .chain(&results.certificate_errors)
            .for_each(super::assert_structured);
        super::assert_diagnostics(&results.diagnostics, document.recovery());
    }
    let errors = views.fields.iter().filter_map(|field| field.error.as_ref());
    errors
        .chain(&views.certification_error)
        .chain(&views.usage_rights_error)
        .for_each(super::assert_structured);
}
