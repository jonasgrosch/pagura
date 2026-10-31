//! `scan`: open and render every page without writing images, and report
//! what opened, rendered, was repaired and failed as one JSON line.

use crate::args::Source;
use crate::failure::Failure;
use crate::json;
use pagura::provider::{Provider, default_provider};
use pagura::{Document, Recovery, RenderContext, RenderOptions};
use std::hint::black_box;
use std::io::{self, Write};
use std::sync::Arc;

/// What a scan found once the document opened.
struct Report {
    /// Whether every step needed no repair.
    strict: bool,
    pages: Option<usize>,
    rendered: usize,
    /// Failures by zero-based page; `None` for the page list itself.
    failures: Vec<(Option<usize>, Failure)>,
}

/// Opens and renders every page, preserving the strict audit without a
/// second pass. A refused repair, while recording or rendering, reopens the
/// same immutable bytes in Repair mode and retries the current page.
/// Earlier strict pages needed no repair; non-repair errors do not make the
/// strict audit fail.
///
/// # Errors
/// Only failures to write the report; everything else is reported.
pub(crate) fn run(
    source: &Source,
    options: &RenderOptions,
    max_pages: Option<usize>,
    out: &mut impl Write,
) -> io::Result<()> {
    let bytes = match source.load() {
        Ok(bytes) => bytes,
        Err(failure) => return open_failure(out, &failure),
    };
    // One provider for every reopening, so its data is parsed once.
    let provider = default_provider(source.options(Recovery::Repair).limits);
    let audited = source
        .open_bytes(Arc::clone(&bytes), Recovery::Strict)
        .and_then(|document| {
            document.pages()?;
            Ok(document)
        });
    let (document, mut strict) = match audited {
        Ok(document) => (document, true),
        Err(_) => match source.open_bytes(Arc::clone(&bytes), Recovery::Repair) {
            Ok(document) => (document, false),
            Err(error) => return open_failure(out, &error.into()),
        },
    };
    let mut context = shared_context(document, provider.as_ref());
    let mut report = Report {
        strict,
        pages: None,
        rendered: 0,
        failures: Vec::new(),
    };
    match context.document().page_count() {
        Ok(count) => report.pages = Some(count),
        Err(error) => report.failures.push((None, error.into())),
    }
    let limit = report
        .pages
        .unwrap_or(0)
        .min(max_pages.unwrap_or(usize::MAX));
    for index in 0..limit {
        let render = |context: &RenderContext| {
            let page = context.document().page(index)?;
            let scene = context.record(page)?;
            context.render_scene(&scene, options, None)
        };
        let mut rendered = render(&context);
        if strict && rendered.as_ref().is_err_and(|error| error.repair.is_some()) {
            let diagnostics = context.document().diagnostics();
            let document = match source.open_bytes(Arc::clone(&bytes), Recovery::Repair) {
                Ok(document) => document,
                Err(error) => return open_failure(out, &error.into()),
            };
            document.report(diagnostics);
            context = shared_context(document, provider.as_ref());
            strict = false;
            rendered = render(&context);
        }
        match rendered {
            Ok(pixmap) => {
                black_box(pixmap);
                report.rendered += 1;
            }
            Err(error) => report.failures.push((Some(index), error.into())),
        }
    }
    report.strict = strict;
    write_report(out, &report, context.document())
}

/// Scans once in Repair mode, without performing or implying a Strict audit.
/// Uses the same source options, provider and store capacity as the default
/// scan. Engine failures, including resource limits, are reported unchanged.
///
/// # Errors
/// Only failures to write the report; everything else is reported.
pub(crate) fn run_repair(
    source: &Source,
    options: &RenderOptions,
    max_pages: Option<usize>,
    out: &mut impl Write,
) -> io::Result<()> {
    let bytes = match source.load() {
        Ok(bytes) => bytes,
        Err(failure) => return repair_open_failure(out, &failure),
    };
    let provider = default_provider(source.options(Recovery::Repair).limits);
    let document = match source.open_bytes(bytes, Recovery::Repair) {
        Ok(document) => document,
        Err(error) => return repair_open_failure(out, &error.into()),
    };
    let context = shared_context(document, provider.as_ref());
    let mut report = Report {
        strict: false,
        pages: None,
        rendered: 0,
        failures: Vec::new(),
    };
    match context.document().page_count() {
        Ok(count) => report.pages = Some(count),
        Err(error) => report.failures.push((None, error.into())),
    }
    let limit = report
        .pages
        .unwrap_or(0)
        .min(max_pages.unwrap_or(usize::MAX));
    for index in 0..limit {
        let rendered = context.document().page(index).and_then(|page| {
            let scene = context.record(page)?;
            context.render_scene(&scene, options, None)
        });
        match rendered {
            Ok(pixmap) => {
                black_box(pixmap);
                report.rendered += 1;
            }
            Err(error) => report.failures.push((Some(index), error.into())),
        }
    }
    write!(
        out,
        "{{\"schema\":{},\"recovery\":\"repair\",\"strict\":null,\"open\":null,\"pages\":",
        json::REPAIR_SCAN_SCHEMA
    )?;
    write_report_body(out, &report, context.document(), true)
}

/// A context for one pass over the pages, sharing `provider` across
/// reopenings.
fn shared_context(document: Document, provider: Option<&Arc<dyn Provider>>) -> RenderContext {
    RenderContext::with_store_capacity(
        Arc::new(document),
        provider.cloned(),
        crate::commands::ONE_PASS_STORE_BYTES,
    )
}

fn write_report(out: &mut impl Write, report: &Report, document: &Document) -> io::Result<()> {
    write!(
        out,
        "{{\"schema\":{},\"strict\":{},\"open\":null,\"pages\":",
        json::SCAN_SCHEMA,
        report.strict
    )?;
    write_report_body(out, report, document, false)
}

fn write_report_body(
    out: &mut impl Write,
    report: &Report,
    document: &Document,
    repair_mode: bool,
) -> io::Result<()> {
    match report.pages {
        Some(count) => write!(out, "{count}")?,
        None => out.write_all(b"null")?,
    }
    write!(out, ",\"rendered\":{},", report.rendered)?;
    json::diagnostics(out, document)?;
    if repair_mode {
        write!(
            out,
            ",\"suppressed_diagnostics\":{}",
            document.suppressed_diagnostics()
        )?;
    }
    out.write_all(b",\"failures\":[")?;
    for (i, (page, failure)) in report.failures.iter().enumerate() {
        if i > 0 {
            out.write_all(b",")?;
        }
        match page {
            Some(page) => write!(out, "{{\"page\":{page},\"error\":")?,
            None => out.write_all(b"{\"page\":null,\"error\":")?,
        }
        json::failure(out, failure)?;
        out.write_all(b"}")?;
    }
    writeln!(out, "]}}")
}

fn open_failure(out: &mut impl Write, failure: &Failure) -> io::Result<()> {
    write!(
        out,
        "{{\"schema\":{},\"strict\":false,\"open\":",
        json::SCAN_SCHEMA
    )?;
    json::failure(out, failure)?;
    writeln!(out, ",\"pages\":null}}")
}

fn repair_open_failure(out: &mut impl Write, failure: &Failure) -> io::Result<()> {
    write!(
        out,
        "{{\"schema\":{},\"recovery\":\"repair\",\"strict\":null,\"open\":",
        json::REPAIR_SCAN_SCHEMA
    )?;
    json::failure(out, failure)?;
    writeln!(
        out,
        ",\"pages\":null,\"rendered\":0,\"repairs\":[],\"information\":[],\"suppressed_diagnostics\":0,\"failures\":[]}}"
    )
}

#[cfg(test)]
mod tests {
    use super::{open_failure, repair_open_failure};
    use crate::failure::Failure;
    use std::io;

    #[test]
    fn unreadable_source_preserves_legacy_bytes_and_has_no_repair_audit() {
        let failure = Failure::Io(io::Error::new(
            io::ErrorKind::NotFound,
            "missing test input",
        ));
        let mut legacy = Vec::new();
        open_failure(&mut legacy, &failure).unwrap();
        assert_eq!(legacy, b"{\"schema\":1,\"strict\":false,\"open\":{\"kind\":\"io\",\"feature\":null,\"repair\":null,\"message\":\"missing test input\"},\"pages\":null}\n");
        let mut repair = Vec::new();
        repair_open_failure(&mut repair, &failure).unwrap();
        assert_eq!(repair, b"{\"schema\":2,\"recovery\":\"repair\",\"strict\":null,\"open\":{\"kind\":\"io\",\"feature\":null,\"repair\":null,\"message\":\"missing test input\"},\"pages\":null,\"rendered\":0,\"repairs\":[],\"information\":[],\"suppressed_diagnostics\":0,\"failures\":[]}\n");
    }
}
