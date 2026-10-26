//! Running a parsed [`Command`]; every result goes to `out`.

use crate::args::{Command, Conversion, HELP, ImageFormat, Output, Source, Timings, USAGE};
use crate::failure::Failure;
use crate::input::{create_new, write_new};
use crate::{document_services, json, microbench, scan};
use pagura::provider::default_provider;
use pagura::text::SearchOptions;
use pagura::{Document, RenderContext, RenderOptions, device_size};
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
        Command::Metadata(source) => document_services::metadata(source, out)?,
        Command::Attachments(source) => document_services::attachments(source, out)?,
        Command::ExtractMetadata { source, output } => {
            document_services::extract_metadata(source, output)?;
        }
        Command::ExtractAttachment {
            source,
            index,
            key,
            output,
        } => {
            document_services::extract_attachment(source, *index, key, output)?;
        }
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
    let pages = document.pages()?;
    write!(
        out,
        "{{\"schema\":{},\"pages\":{},\"objects\":{},\"bytes\":{},\"page_geometry\":[",
        json::INSPECT_SCHEMA,
        pages.len(),
        document.object_count(),
        document.original_bytes().len(),
    )?;
    for (index, page) in pages.iter().enumerate() {
        if index > 0 {
            out.write_all(b",")?;
        }
        out.write_all(b"{\"media_box\":")?;
        json::rectangle(out, page.media_box())?;
        out.write_all(b",\"crop_box\":")?;
        json::rectangle(out, page.crop_box())?;
        write!(
            out,
            ",\"rotation\":{},\"user_unit\":{}}}",
            page.rotation().degrees(),
            page.user_unit(),
        )?;
    }
    out.write_all(b"],")?;
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
    let mut stages = Vec::with_capacity(indices.clone().count());
    for page in indices {
        let path = match &conversion.output {
            Output::File(path) => path.clone(),
            Output::Directory(directory) => image_path(directory, page, conversion.format),
        };
        stages.push(match conversion.format {
            ImageFormat::Ppm => convert_ppm(&context, page, &conversion.render, &path)?,
            ImageFormat::Png => convert_png(&context, page, &conversion.render, &path)?,
        });
    }
    if conversion.timings == Timings::Print {
        write_timings(out, open, &stages, started.elapsed())?;
    }
    Ok(())
}

/// PPM stays on the existing bounded band-output path.
fn convert_ppm(
    context: &RenderContext,
    page: usize,
    options: &RenderOptions,
    path: &Path,
) -> Result<Stage, Failure> {
    let page_started = Instant::now();
    let emit_started = Instant::now();
    #[expect(
        clippy::large_stack_arrays,
        reason = "one fixed 48 KiB output packing buffer, independent of input dimensions"
    )]
    let mut rgb = [0; RGB_CHUNK_BYTES];
    let mut used = 0;
    let mut file: Option<File> = None;
    let mut emit = emit_started.elapsed();
    // Each page is visited once: its scene is dropped after rendering
    // instead of being stored.
    let source = context.document().page(page)?;
    let scene = context.record(source)?;
    let size = device_size(source, options)?;
    let rendered = context.render_bands(
        &scene,
        options,
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
                        let mut opened = create_new(path)?;
                        write_ppm_header(&mut opened, size)?;
                        empty.insert(opened)
                    }
                };
                write_rgb(file, pixels, &mut rgb, &mut used)
            })();
            emit += emit_started.elapsed();
            written
        },
    );
    drop(scene);
    let emit_started = Instant::now();
    // A successful stop keeps its completed bands. On any earlier engine
    // or consumer error, discard the tail without another writer call.
    let finished = if matches!(&rendered, Ok(Ok(_))) {
        file.as_mut()
            .map_or(Ok(()), |file| drain_rgb(file, &rgb, &mut used))
    } else {
        Ok(())
    };
    drop(file);
    emit += emit_started.elapsed();
    // Sink work is interleaved with execution. Measure all of it in
    // emit, including conversion and file close, rather than render.
    let render = page_started.elapsed().saturating_sub(emit);
    rendered??;
    finished?;
    Ok(Stage { page, render, emit })
}

/// PNG currently collects a complete surface, then encodes before creating output.
fn convert_png(
    context: &RenderContext,
    page: usize,
    options: &RenderOptions,
    path: &Path,
) -> Result<Stage, Failure> {
    let started = Instant::now();
    let page_source = context.document().page(page)?;
    let scene = context.record(page_source)?;
    let surface = context.render_scene(&scene, options, None)?;
    drop(scene);
    let render = started.elapsed();
    let started = Instant::now();
    let bytes = pagura::png::encode_pixmap(
        surface,
        pagura::png::Level::Default,
        &context.document().limits(),
    )?;
    write_new(path, &bytes)?;
    Ok(Stage {
        page,
        render,
        emit: started.elapsed(),
    })
}

/// `page-NNNN.FORMAT` for zero-based `page`, numbered from 1 as pdftoppm names
/// its output, which the benchmark harness compares against.
fn image_path(directory: &Path, page: usize, format: ImageFormat) -> std::path::PathBuf {
    directory.join(format!("page-{:04}.{}", page + 1, format.extension()))
}

/// The sole fixed packing buffer, shared across bands and independent of
/// page dimensions. Its complete chunks are written without a second copy.
const RGB_CHUNK_BYTES: usize = 3 * 16_384;

/// Binary PPM (P6) starts with the dimensions of the complete page.
fn write_ppm_header(out: &mut impl Write, (width, height): (u32, u32)) -> std::io::Result<()> {
    write!(out, "P6\n{width} {height}\n255\n")
}

/// Packs already-quantized opaque output, with no second compositing or
/// rounding step. The CLI selects opaque white for PPM before rendering.
fn write_rgb(
    out: &mut impl Write,
    rgba: &[u8],
    rgb: &mut [u8; RGB_CHUNK_BYTES],
    used: &mut usize,
) -> std::io::Result<()> {
    let (mut pixels, rest) = rgba.as_chunks::<4>();
    debug_assert_eq!(rest, []);
    debug_assert!(*used <= rgb.len() && used.is_multiple_of(3));
    while !pixels.is_empty() {
        let count = pixels.len().min((rgb.len() - *used) / 3);
        let (next, remaining) = pixels.split_at(count);
        for (target, pixel) in rgb[*used..][..count * 3]
            .as_chunks_mut::<3>()
            .0
            .iter_mut()
            .zip(next)
        {
            debug_assert_eq!(pixel[3], 255);
            target.copy_from_slice(&pixel[..3]);
        }
        *used += count * 3;
        if *used == rgb.len() {
            drain_rgb(out, rgb, used)?;
        }
        pixels = remaining;
    }
    Ok(())
}

/// Drains only on explicit success paths. There is no Drop-time writer.
fn drain_rgb(
    out: &mut impl Write,
    rgb: &[u8; RGB_CHUNK_BYTES],
    used: &mut usize,
) -> std::io::Result<()> {
    if *used != 0 {
        out.write_all(&rgb[..*used])?;
        *used = 0;
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
mod tests;
