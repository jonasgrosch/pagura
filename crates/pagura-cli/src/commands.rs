//! Running a parsed [`Command`]; every result goes to `out`.

use crate::args::{Command, Conversion, HELP, Output, Source, Timings, USAGE};
use crate::failure::Failure;
use crate::input::write_new;
use crate::{json, microbench, scan};
use pagura::text::SearchOptions;
use pagura::{Document, Pixmap, Rgba8};
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Runs `command`, writing its output to `out`.
///
/// # Errors
/// The first [`Failure`]; output written before it stays written.
pub(crate) fn run(command: &Command, out: &mut impl Write) -> Result<(), Failure> {
    match command {
        Command::Help => writeln!(out, "{USAGE}\n\n{HELP}")?,
        Command::Version => writeln!(out, "pagura {} (safe Rust)", env!("CARGO_PKG_VERSION"))?,
        Command::Licenses => licenses(out)?,
        Command::Inspect(source) => inspect(source, out)?,
        Command::Text { source, page } => text(source, *page, out)?,
        Command::Find { source, query } => find(source, query, out)?,
        Command::Convert(conversion) => convert(conversion, out)?,
        Command::Scan {
            source,
            render,
            max_pages,
        } => scan::run(source, render, *max_pages, out)?,
        Command::Roundtrip { source, output } => {
            let document = source.open()?;
            document.pages()?;
            write_new(output, document.original_bytes())?;
        }
        Command::Microbench {
            source,
            operation,
            count,
            render,
        } => microbench::run(source, *operation, *count, render, out)?,
    }
    Ok(())
}

fn licenses(out: &mut impl Write) -> std::io::Result<()> {
    for (subject, notice) in pagura::notices() {
        writeln!(out, "{subject}\n\n{notice}")?;
    }
    if !pagura::provider::BUNDLED {
        writeln!(out, "Font and CMap data: No bundled data is linked.")?;
    }
    Ok(())
}

fn inspect(source: &Source, out: &mut impl Write) -> Result<(), Failure> {
    let document = source.open()?;
    let pages = document.page_count()?;
    write!(
        out,
        "{{\"schema\":{},\"pages\":{pages},\"objects\":{},\"bytes\":{},",
        json::INSPECT_SCHEMA,
        document.object_count(),
        document.original_bytes().len(),
    )?;
    json::diagnostics(out, &document)?;
    writeln!(out, "}}")?;
    Ok(())
}

/// The zero-based pages to visit: `selected`, or every page.
fn pages(document: &Document, selected: Option<usize>) -> Result<std::ops::Range<usize>, Failure> {
    let count = document.page_count()?;
    match selected {
        Some(page) if page >= count => Err(Failure::Request(format!(
            "page {page} outside the document's {count} pages"
        ))),
        Some(page) => Ok(page..page + 1),
        None => Ok(0..count),
    }
}

/// Prints the text of the selected pages, separated by form feeds.
fn text(source: &Source, page: Option<usize>, out: &mut impl Write) -> Result<(), Failure> {
    let context = pagura::render_context(Arc::new(source.open()?));
    let document = context.document();
    for (ordinal, index) in pages(document, page)?.enumerate() {
        let text = context.text_page(document.page(index)?)?;
        if ordinal > 0 {
            out.write_all(b"\x0c\n")?;
        }
        out.write_all(text.text().as_bytes())?;
        out.write_all(b"\n")?;
    }
    Ok(())
}

/// Prints one JSON line per match, with its page, character range and
/// quads in default user space.
fn find(source: &Source, query: &str, out: &mut impl Write) -> Result<(), Failure> {
    let context = pagura::render_context(Arc::new(source.open()?));
    let document = context.document();
    for index in pages(document, None)? {
        let text = context.text_page(document.page(index)?)?;
        for found in text.find(query, SearchOptions::default())? {
            let (start, end) = (found.range.start, found.range.end);
            write!(
                out,
                "{{\"page\":{index},\"start\":{start},\"end\":{end},\"quads\":["
            )?;
            for (i, quad) in found.quads.iter().enumerate() {
                out.write_all(if i > 0 { b",[" } else { b"[" })?;
                for (j, point) in quad.0.iter().enumerate() {
                    let separator = if j > 0 { "," } else { "" };
                    write!(out, "{separator}[{},{}]", point.x, point.y)?;
                }
                out.write_all(b"]")?;
            }
            writeln!(out, "]}}")?;
        }
    }
    Ok(())
}

/// The time one page took to render and to write.
struct Stage {
    page: usize,
    render: Duration,
    emit: Duration,
}

/// Converts the zero-based inclusive pages (default: all) to images. The
/// whole range is validated against the page list before anything is written.
fn convert(conversion: &Conversion, out: &mut impl Write) -> Result<(), Failure> {
    let started = Instant::now();
    let context = pagura::render_context(Arc::new(conversion.source.open()?));
    let document = context.document();
    let count = document.page_count()?;
    let indices = conversion
        .pages
        .clone()
        .unwrap_or(0..=count.saturating_sub(1));
    if count == 0 || indices.is_empty() || *indices.end() >= count {
        return Err(Failure::Request(format!(
            "page range outside the document's {count} pages"
        )));
    }
    let open = started.elapsed();
    let options = conversion.render;
    let mut stages = Vec::with_capacity(indices.clone().count());
    for page in indices {
        let render_started = Instant::now();
        let pixmap = context.render_page(document.page(page)?, &options)?;
        let render = render_started.elapsed();
        let emit_started = Instant::now();
        let path = match &conversion.output {
            Output::File(path) => path.clone(),
            Output::Directory(directory) => image_path(directory, page),
        };
        write_new(&path, &encode_ppm(&pixmap))?;
        stages.push(Stage {
            page,
            render,
            emit: emit_started.elapsed(),
        });
    }
    if conversion.timings == Timings::Print {
        write_timings(out, open, &stages, started.elapsed())?;
    }
    Ok(())
}

/// `page-NNNN.ppm` for zero-based `page`, numbered from 1 as pdftoppm names
/// its output, which the benchmark harness compares against.
fn image_path(directory: &Path, page: usize) -> std::path::PathBuf {
    directory.join(format!("page-{:04}.ppm", page + 1))
}

/// Binary PPM (P6) over white: the simplest raw image format.
fn encode_ppm(pixmap: &Pixmap) -> Vec<u8> {
    let mut ppm = format!("P6\n{} {}\n255\n", pixmap.width(), pixmap.height()).into_bytes();
    ppm.extend(pixmap.to_rgb_over(Rgba8::WHITE));
    ppm
}

fn write_timings(
    out: &mut impl Write,
    open: Duration,
    stages: &[Stage],
    total: Duration,
) -> std::io::Result<()> {
    write!(
        out,
        "{{\"schema\":{},\"open_ns\":{},\"pages\":[",
        json::TIMINGS_SCHEMA,
        open.as_nanos()
    )?;
    for (i, stage) in stages.iter().enumerate() {
        write!(
            out,
            "{}{{\"page\":{},\"render_ns\":{},\"emit_ns\":{}}}",
            if i > 0 { "," } else { "" },
            stage.page,
            stage.render.as_nanos(),
            stage.emit.as_nanos()
        )?;
    }
    writeln!(out, "],\"total_ns\":{}}}", total.as_nanos())
}

#[cfg(test)]
mod tests {
    use super::run;
    use crate::args::{Command, Source};
    use crate::failure::Failure;
    use pagura::Recovery;
    use pagura_testkit::fixture_path;

    fn source(area: &str, file: &str, recovery: Recovery) -> Source {
        Source {
            path: fixture_path(area, file),
            password: Vec::new(),
            recovery,
        }
    }

    fn output(command: &Command) -> Result<String, Failure> {
        let mut out = Vec::new();
        run(command, &mut out)?;
        Ok(String::from_utf8(out).unwrap())
    }

    #[test]
    fn inspect_reports_counts_and_diagnostics_as_one_json_line() {
        let trailing = source("classic", "trailing-bytes.pdf", Recovery::Repair);
        let line = output(&Command::Inspect(trailing.clone())).unwrap();
        assert!(line.starts_with("{\"schema\":2,\"pages\":1,"), "{line}");
        assert!(
            line.ends_with(",\"repairs\":[\"file.trailing-bytes\"],\"information\":[]}\n"),
            "{line}"
        );
        let strict = Source {
            recovery: Recovery::Strict,
            ..trailing
        };
        let Err(Failure::Engine(error)) = output(&Command::Inspect(strict)) else {
            panic!("strict inspect accepted trailing bytes");
        };
        assert_eq!(
            error.repair.map(pagura::Repair::as_str),
            Some("file.trailing-bytes")
        );
    }

    #[test]
    fn text_rejects_a_page_outside_the_document_before_printing() {
        let command = Command::Text {
            source: source("text", "extract-tj.pdf", Recovery::Repair),
            page: Some(1),
        };
        let Err(Failure::Request(message)) = output(&command) else {
            panic!("page 1 of a one-page document");
        };
        assert_eq!(message, "page 1 outside the document's 1 pages");
    }

    #[test]
    fn help_version_and_licenses_print_to_the_output() {
        assert!(
            output(&Command::Help)
                .unwrap()
                .contains("Exit status: 0 on")
        );
        assert!(output(&Command::Version).unwrap().starts_with("pagura "));
        let licenses = output(&Command::Licenses).unwrap();
        for (subject, _) in pagura::notices() {
            assert!(licenses.contains(subject), "{subject}");
        }
    }
}
