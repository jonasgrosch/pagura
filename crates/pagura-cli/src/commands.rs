//! Running a parsed [`Command`]; every result goes to `out`.

use crate::args::{Command, Conversion, HELP, Output, Source, Timings, USAGE};
use crate::failure::Failure;
use crate::input::{create_new, write_new};
use crate::{json, microbench, scan};
use pagura::provider::default_provider;
use pagura::text::SearchOptions;
use pagura::{Document, RenderContext, RenderOptions, Rgba8, device_size};
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The store capacity of the commands that visit each page once (`text`,
/// `find`, `render`, `convert` and `scan`).
///
/// They drop each page's scene after the page, so the store keeps only what
/// later pages reuse: font programs, glyph edge setups, colour transforms and
/// repeated images. 16 MiB holds the fonts of typical documents while
/// bounding what the decoded images of earlier pages add to peak memory; the
/// previous renderer kept fonts and one page surface between pages and no
/// decoded image when rendering exactly. A font program larger than this is
/// parsed again for every page instead of being kept.
pub(crate) const ONE_PASS_STORE_BYTES: usize = 16 * 1024 * 1024;

/// A context for one pass over the pages of `document`, with the
/// feature-selected font data and [`ONE_PASS_STORE_BYTES`].
pub(crate) fn one_pass_context(document: Document) -> RenderContext {
    let provider = default_provider(document.limits());
    RenderContext::with_store_capacity(Arc::new(document), provider, ONE_PASS_STORE_BYTES)
}

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
    let context = one_pass_context(source.open()?);
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
    let context = one_pass_context(source.open()?);
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
    let context = one_pass_context(conversion.source.open()?);
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
    // PPM has no alpha. The renderer composites onto opaque white before
    // quantizing, so its final RGB bytes can be written directly.
    let options = RenderOptions {
        background: Rgba8::WHITE,
        ..conversion.render
    };
    let mut stages = Vec::with_capacity(indices.clone().count());
    for page in indices {
        let page_started = Instant::now();
        let emit_started = Instant::now();
        let path = match &conversion.output {
            Output::File(path) => path.clone(),
            Output::Directory(directory) => image_path(directory, page),
        };
        let mut rgb = [0; RGB_CHUNK_BYTES];
        let mut file: Option<File> = None;
        let mut emit = emit_started.elapsed();
        // Each page is visited once: its scene is dropped after rendering
        // instead of being stored.
        let source = document.page(page)?;
        let scene = context.record(source)?;
        let size = device_size(source, &options)?;
        let rendered = context.render_bands(
            &scene,
            &options,
            None,
            |_| false,
            |_, pixels| {
                let emit_started = Instant::now();
                let written = (|| {
                    // The first callback follows preparation, strict
                    // repair checks and every renderer allocation.
                    let file = match &mut file {
                        Some(file) => file,
                        empty => {
                            let mut opened = create_new(&path)?;
                            write_ppm_header(&mut opened, size)?;
                            empty.insert(opened)
                        }
                    };
                    write_rgb(file, pixels, &mut rgb)
                })();
                emit += emit_started.elapsed();
                written
            },
        );
        drop(scene);
        let emit_started = Instant::now();
        drop(file);
        emit += emit_started.elapsed();
        // Sink work is interleaved with execution. Measure all of it in
        // emit, including conversion and file close, rather than render.
        let render = page_started.elapsed().saturating_sub(emit);
        rendered??;
        stages.push(Stage { page, render, emit });
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

/// A fixed encoding chunk, independent of the page or band dimensions.
const RGB_CHUNK_BYTES: usize = 3 * 4096;

/// Binary PPM (P6) starts with the dimensions of the complete page.
fn write_ppm_header(out: &mut impl Write, (width, height): (u32, u32)) -> std::io::Result<()> {
    write!(out, "P6\n{width} {height}\n255\n")
}

/// Packs already-quantized opaque output, with no second compositing or
/// rounding step. Opaque white is fixed at the CLI's RenderOptions above.
fn write_rgb(
    out: &mut impl Write,
    rgba: &[u8],
    rgb: &mut [u8; RGB_CHUNK_BYTES],
) -> std::io::Result<()> {
    let (pixels, rest) = rgba.as_chunks::<4>();
    debug_assert_eq!(rest, []);
    for pixels in pixels.chunks(rgb.len() / 3) {
        for (target, pixel) in rgb.as_chunks_mut::<3>().0.iter_mut().zip(pixels) {
            debug_assert_eq!(pixel[3], 255);
            target.copy_from_slice(&pixel[..3]);
        }
        out.write_all(&rgb[..pixels.len() * 3])?;
    }
    Ok(())
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
    use super::{RGB_CHUNK_BYTES, run, write_ppm_header, write_rgb};
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
    fn streamed_ppm_header_and_payload_keep_pixel_order_across_chunks_and_calls() {
        let mut actual = Vec::new();
        let mut rgb = [0; RGB_CHUNK_BYTES];
        write_ppm_header(&mut actual, (3, 2)).unwrap();
        write_rgb(&mut actual, &[1, 2, 3, 255, 11, 22, 33, 255], &mut rgb).unwrap();
        write_rgb(
            &mut actual,
            &[
                101, 102, 103, 255, 10, 20, 30, 255, 0, 127, 255, 255, 255, 254, 253, 255,
            ],
            &mut rgb,
        )
        .unwrap();
        assert_eq!(actual, b"P6\n3 2\n255\n\x01\x02\x03\x0b\x16\x21\x65\x66\x67\x0a\x14\x1e\x00\x7f\xff\xff\xfe\xfd");
        let pixels: Vec<u8> = [[1, 2, 3, 255], [101, 102, 103, 255]]
            .into_iter()
            .cycle()
            .take(RGB_CHUNK_BYTES / 3 + 3)
            .flatten()
            .collect();
        actual.clear();
        write_rgb(&mut actual, &pixels, &mut rgb).unwrap();
        let expected = pagura::Pixmap::from_premultiplied(
            u32::try_from(pixels.len() / 4).unwrap(),
            1,
            pixels,
            pagura::Limits::default(),
        )
        .unwrap()
        .to_rgb_over(pagura::Rgba8::WHITE);
        assert_eq!(actual, expected);
    }

    #[test]
    fn a_failed_write_stops_rgb_encoding() {
        struct Failed;
        impl std::io::Write for Failed {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("sink failed"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut rgb = [0; RGB_CHUNK_BYTES];
        let error = write_rgb(&mut Failed, &[1, 2, 3, 255], &mut rgb).unwrap_err();
        assert_eq!(error.to_string(), "sink failed");
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
