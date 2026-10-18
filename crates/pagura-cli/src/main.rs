#![forbid(unsafe_code)]
//! The `pagura` command-line tool: inspection, PDF-to-image conversion,
//! corpus scanning and internal profiling.

mod password;

use pagura::{
    Accuracy, Document, Limits, OpenOptions, OverprintPreview, Pixmap, Recovery, RenderOptions,
    Rgba8,
};
use std::error::Error;
use std::fmt::Write as _;
use std::fs::File;
use std::hint::black_box;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, Instant};

type CliResult<T = ()> = Result<T, Box<dyn Error>>;

const USAGE: &str = "usage: pagura inspect [--strict] INPUT
       pagura licenses
       pagura render [--timings] INPUT OUTPUT.ppm [DPI=72] [PAGE=0]
       pagura convert [--timings] INPUT OUTPUT_DIR [DPI=72] [FIRST LAST]
       pagura text INPUT [PAGE]
       pagura find INPUT QUERY
       pagura scan [--max-pages N] INPUT [DPI=72]
       pagura roundtrip INPUT OUTPUT
       pagura microbench INPUT open-pages|render-cold|render-warm COUNT [DPI=72]

options:
  --password TEXT  password for encrypted documents (default: empty)
  --strict         reject malformed input instead of repairing it
  --timings        print JSON stage timings to stdout: open (read and parse
                   the file, build the page list), render (rasterize into
                   memory) and emit (encode and write the image); the
                   engine's contribution to a conversion is open + render
  --overprint MODE auto (default): PDF/X declarations; always: simulate; never: ignore
  --accuracy MODE  exact (default): every pixel within one level of exact;
                   competitive: the approximate path of RenderOptions::accuracy
  --max-pages N    scan at most N pages per document
scan prints one JSON line describing what opened and rendered, never images.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("pagura: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Options that may appear anywhere on the command line.
#[derive(Debug, Clone, Default)]
struct Flags {
    #[cfg(feature = "bundled-data")]
    provider: std::sync::Arc<pagura_data::Bundled>,
    strict: bool,
    timings: bool,
    password: Vec<u8>,
    max_pages: Option<usize>,
    overprint: OverprintPreview,
    accuracy: Accuracy,
}

impl Flags {
    fn open_options(&self) -> OpenOptions {
        OpenOptions {
            recovery: if self.strict {
                Recovery::Strict
            } else {
                Recovery::Repair
            },
            password: self.password.clone(),
            ..OpenOptions::default()
        }
    }

    fn render_options(&self, dpi: f64) -> RenderOptions {
        RenderOptions {
            overprint: self.overprint,
            accuracy: self.accuracy,
            ..RenderOptions::from_dpi(dpi)
        }
    }

    fn with_strict(&self, strict: bool) -> Self {
        Self {
            strict,
            ..self.clone()
        }
    }
}

/// Splits flags from positional arguments.
fn parse(args: &[String]) -> CliResult<(Flags, Vec<&str>)> {
    let mut flags = Flags::default();
    let mut positional = Vec::new();
    let mut args = args.iter().map(String::as_str);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg {
            "--strict" => flags.strict = true,
            "--timings" => flags.timings = true,
            "--password" => flags.password = value()?.as_bytes().to_vec(),
            "--max-pages" => flags.max_pages = Some(value()?.parse()?),
            "--overprint" => {
                flags.overprint = match value()? {
                    "auto" => OverprintPreview::Auto,
                    "always" => OverprintPreview::Always,
                    "never" => OverprintPreview::Never,
                    _ => return Err("--overprint requires auto, always or never".into()),
                };
            }
            "--accuracy" => {
                flags.accuracy = match value()? {
                    "exact" => Accuracy::Exact,
                    "competitive" => Accuracy::Competitive,
                    _ => return Err("--accuracy requires exact or competitive".into()),
                };
            }
            "--version" => positional.push(arg),
            flag if flag.starts_with("--") => {
                return Err(format!("unknown option {flag}\n{USAGE}").into());
            }
            _ => positional.push(arg),
        }
    }
    Ok((flags, positional))
}

fn dpi(value: Option<&&str>) -> CliResult<f64> {
    Ok(value.map_or(Ok(72.0), |dpi| dpi.parse::<f64>())?)
}

fn run(args: &[String]) -> CliResult {
    let (flags, args) = parse(args)?;
    match args.as_slice() {
        ["licenses"] => {
            let mut found = false;
            for (subject, notice) in pagura::data_notices() {
                println!("{subject}\n\n{notice}");
                found = true;
            }
            println!("Unicode 3.2.0\n\n{}", pagura_crypto::unicode_notice());
            if !found {
                println!("Font and CMap data: No bundled data is linked.");
            }
            Ok(())
        }
        ["--version"] => {
            println!("pagura {} (safe Rust)", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        ["inspect", input] => inspect(input, &flags),
        ["text", input, rest @ ..] if rest.len() <= 1 => {
            let page = rest.first().map(|p| p.parse::<usize>()).transpose()?;
            extract(input, page, None, &flags)
        }
        ["find", input, query] => extract(input, None, Some(query), &flags),
        ["render", input, output, rest @ ..] if rest.len() <= 2 => {
            let page = rest.get(1).map_or(Ok(0), |page| page.parse::<usize>())?;
            let output = Output::File(output);
            convert(input, output, dpi(rest.first())?, Some(page..=page), &flags)
        }
        ["convert", input, output, rest @ ..] if matches!(rest.len(), 0 | 1 | 3) => {
            let pages = match rest {
                [_, first, last] => Some(first.parse::<usize>()?..=last.parse::<usize>()?),
                _ => None,
            };
            let output = Output::Directory(output);
            convert(input, output, dpi(rest.first())?, pages, &flags)
        }
        ["scan", input, rest @ ..] if rest.len() <= 1 => {
            println!("{}", scan(input, dpi(rest.first())?, &flags));
            Ok(())
        }
        ["roundtrip", input, output] => {
            let document = open(input, &flags.with_strict(true))?;
            document.pages()?;
            write_new(Path::new(output), document.original_bytes())
        }
        ["microbench", input, operation, count, rest @ ..] if rest.len() <= 1 => {
            microbench(input, operation, count.parse()?, dpi(rest.first())?, &flags)
        }
        _ => Err(USAGE.into()),
    }
}

/// Reads a file within the size budget. A regular file is read once into
/// storage of its length; anything else, or a file whose length changes while
/// it is read, is read again to its end.
fn load(path: &str, limits: Limits) -> CliResult<std::sync::Arc<[u8]>> {
    let mut file = File::open(path)?;
    let limit = u64::try_from(limits.file_bytes)?;
    let metadata = file.metadata()?;
    if metadata.len() > limit {
        return Err("input exceeds the file size budget".into());
    }
    if metadata.is_file() {
        let length = usize::try_from(metadata.len())?;
        let mut bytes: std::sync::Arc<[u8]> = std::iter::repeat_n(0, length).collect();
        let buffer = std::sync::Arc::get_mut(&mut bytes).ok_or("input storage is shared")?;
        match file.read_exact(buffer) {
            Ok(()) if file.read(&mut [0])? == 0 => return Ok(bytes),
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {}
            Err(error) => return Err(error.into()),
        }
        file.seek(SeekFrom::Start(0))?;
    }
    let mut data = Vec::new();
    file.take(limit + 1).read_to_end(&mut data)?;
    if data.len() > limits.file_bytes {
        return Err("input grew beyond the file size budget".into());
    }
    Ok(data.into())
}

fn open(path: &str, flags: &Flags) -> CliResult<Document> {
    open_bytes(load(path, flags.open_options().limits)?, flags)
}

fn open_bytes(bytes: std::sync::Arc<[u8]>, flags: &Flags) -> CliResult<Document> {
    let options = flags.open_options();
    let document = password::open(bytes, &options)?;
    Ok(document)
}

fn rendering(document: std::sync::Arc<Document>, flags: &Flags) -> pagura::RenderContext {
    #[cfg(feature = "bundled-data")]
    return pagura::RenderContext::with_provider(document, flags.provider.clone());
    #[cfg(not(feature = "bundled-data"))]
    {
        let _ = flags;
        pagura::RenderContext::new(document)
    }
}

/// Page numbers are zero-based, as for render. Text separates pages with FF;
/// find emits JSON lines with byte ranges and default-user-space quads.
fn extract(input: &str, selected: Option<usize>, query: Option<&str>, flags: &Flags) -> CliResult {
    let context = rendering(std::sync::Arc::new(open(input, flags)?), flags);
    let document = context.document();
    let count = document.page_count()?;
    if selected.is_some_and(|p| p >= count) {
        return Err("page index outside document".into());
    }
    let range = selected.map_or(0..count, |p| p..p + 1);
    let mut stdout = std::io::stdout().lock();
    for (ordinal, index) in range.enumerate() {
        let text = context.text_page(document.page(index)?)?;
        if let Some(query) = query {
            for found in text.find(query, pagura::SearchOptions::default())? {
                let mut quads = String::new();
                for (i, quad) in found.quads.iter().enumerate() {
                    if i > 0 {
                        quads.push(',');
                    }
                    quads.push('[');
                    for (j, point) in quad.0.iter().enumerate() {
                        if j > 0 {
                            quads.push(',');
                        }
                        write!(quads, "[{},{}]", point.x, point.y)?;
                    }
                    quads.push(']');
                }
                writeln!(
                    stdout,
                    "{{\"page\":{index},\"start\":{},\"end\":{},\"quads\":[{quads}]}}",
                    found.range.start, found.range.end
                )?;
            }
        } else {
            if ordinal > 0 {
                stdout.write_all(b"\x0c\n")?;
            }
            stdout.write_all(text.text().as_bytes())?;
            stdout.write_all(b"\n")?;
        }
    }
    Ok(())
}

/// Creates `path` and writes `bytes`; never truncates an existing file.
fn write_new(path: &Path, bytes: &[u8]) -> CliResult {
    let mut file = File::options().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    Ok(())
}

fn repair_tags(document: &Document) -> String {
    let tags: Vec<String> = document
        .diagnostics()
        .iter()
        .filter_map(pagura_core::Diagnostic::repair)
        .map(|repair| json_string(repair.as_str()))
        .collect();
    tags.join(",")
}

fn information_tags(document: &Document) -> String {
    document
        .diagnostics()
        .iter()
        .filter_map(|item| match item.kind {
            pagura::DiagnosticKind::Information(tag) => Some(json_string(tag)),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn inspect(input: &str, flags: &Flags) -> CliResult {
    let document = open(input, flags)?;
    let pages = document.page_count()?;
    println!(
        "{{\"schema\":2,\"pages\":{pages},\"objects\":{},\"bytes\":{},\"repairs\":[{}],\"information\":[{}]}}",
        document.object_count(),
        document.original_bytes().len(),
        repair_tags(&document),
        information_tags(&document)
    );
    Ok(())
}

/// Where converted pages go.
#[derive(Debug, Clone, Copy)]
enum Output<'a> {
    /// One page to one file.
    File(&'a str),
    /// Every page to `page-NNNN.ppm` in an existing directory.
    Directory(&'a str),
}

/// Binary PPM (P6) over white: the simplest raw image format.
fn encode_ppm(pixmap: &Pixmap) -> Vec<u8> {
    let mut ppm = format!("P6\n{} {}\n255\n", pixmap.width(), pixmap.height()).into_bytes();
    ppm.extend(pixmap.to_rgb_over(Rgba8::WHITE));
    ppm
}

/// Converts the zero-based inclusive `pages` (default: all) to images. The
/// whole range is validated against the page list before anything is written.
fn convert(
    input: &str,
    output: Output<'_>,
    dpi: f64,
    pages: Option<std::ops::RangeInclusive<usize>>,
    flags: &Flags,
) -> CliResult {
    let started = Instant::now();
    let document = std::sync::Arc::new(open(input, flags)?);
    let context = rendering(document.clone(), flags);
    let count = document.page_count()?;
    let indices = pages.unwrap_or(0..=count.saturating_sub(1));
    if count == 0 || indices.is_empty() || *indices.end() >= count {
        return Err(format!("page range outside the document's {count} pages").into());
    }
    let open_time = started.elapsed();
    let options = flags.render_options(dpi);
    let mut stages = Vec::with_capacity(indices.clone().count());
    for index in indices {
        let render_started = Instant::now();
        let pixmap = context.render_page(document.page(index)?, &options)?;
        let render_time = render_started.elapsed();
        let emit_started = Instant::now();
        let path = match output {
            Output::File(path) => path.into(),
            Output::Directory(directory) => {
                Path::new(directory).join(format!("page-{:04}.ppm", index + 1))
            }
        };
        write_new(&path, &encode_ppm(&pixmap))?;
        context.recycle(pixmap);
        stages.push((index, render_time, emit_started.elapsed()));
    }
    if flags.timings {
        print_timings(open_time, &stages, started.elapsed());
    }
    Ok(())
}

fn print_timings(open: Duration, pages: &[(usize, Duration, Duration)], total: Duration) {
    let pages: Vec<String> = pages
        .iter()
        .map(|(index, render, emit)| {
            format!(
                "{{\"page\":{index},\"render_ns\":{},\"emit_ns\":{}}}",
                render.as_nanos(),
                emit.as_nanos()
            )
        })
        .collect();
    println!(
        "{{\"schema\":1,\"open_ns\":{},\"pages\":[{}],\"total_ns\":{}}}",
        open.as_nanos(),
        pages.join(","),
        total.as_nanos()
    );
}

/// A JSON string literal.
fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A JSON object describing an error.
fn json_error(error: &(dyn Error + 'static)) -> String {
    match error.downcast_ref::<pagura::Error>() {
        Some(error) => format!(
            "{{\"kind\":\"{}\",\"feature\":{},\"repair\":{},\"message\":{}}}",
            error.kind.name(),
            error
                .feature()
                .map_or_else(|| "null".into(), |feature| json_string(feature.as_str())),
            error
                .repair
                .map_or_else(|| "null".into(), |repair| json_string(repair.as_str())),
            json_string(error.message)
        ),
        None => format!(
            "{{\"kind\":\"io\",\"feature\":null,\"repair\":null,\"message\":{}}}",
            json_string(&error.to_string())
        ),
    }
}

/// Opens and renders every page, preserving the strict audit without a
/// second compilation pass. A refused repair reopens the same immutable bytes
/// in Repair mode and retries the current page. Earlier strict compilations
/// needed no repair; non-repair errors do not make the strict audit fail.
fn scan(input: &str, dpi: f64, flags: &Flags) -> String {
    let bytes = match load(input, flags.open_options().limits) {
        Ok(bytes) => bytes,
        Err(error) => return scan_open_error(&*error),
    };
    let repair_flags = flags.with_strict(false);
    let mut strict = true;
    let clean = open_bytes(bytes.clone(), &flags.with_strict(true)).and_then(|document| {
        document.pages()?;
        Ok(document)
    });
    let document = if let Ok(document) = clean {
        document
    } else {
        strict = false;
        match open_bytes(bytes.clone(), &repair_flags) {
            Ok(document) => document,
            Err(error) => return scan_open_error(&*error),
        }
    };
    let mut context = rendering(std::sync::Arc::new(document), flags);
    let options = flags.render_options(dpi);
    let mut failures = Vec::new();
    let pages = context
        .document()
        .page_count()
        .inspect_err(|error| {
            failures.push(format!("{{\"page\":null,\"error\":{}}}", json_error(error)));
        })
        .ok();
    let limit = pages
        .unwrap_or(0)
        .min(flags.max_pages.unwrap_or(usize::MAX));
    let mut rendered = 0_usize;
    for index in 0..limit {
        let mut list = context
            .document()
            .page(index)
            .and_then(|page| context.compile_page(page));
        if strict && list.as_ref().is_err_and(|error| error.repair.is_some()) {
            let diagnostics = context.document().diagnostics();
            let document = match open_bytes(bytes.clone(), &repair_flags) {
                Ok(document) => document,
                Err(error) => return scan_open_error(&*error),
            };
            document.report(diagnostics);
            context = rendering(std::sync::Arc::new(document), flags);
            strict = false;
            list = context
                .document()
                .page(index)
                .and_then(|page| context.compile_page(page));
        }
        let result = list.and_then(|list| list.render(&options));
        match result {
            Ok(pixmap) => {
                black_box(pixmap);
                rendered += 1;
            }
            Err(error) => failures.push(format!(
                "{{\"page\":{index},\"error\":{}}}",
                json_error(&error)
            )),
        }
    }
    format!(
        "{{\"schema\":1,\"strict\":{strict},\"open\":null,\"pages\":{},\"rendered\":{rendered},\
         \"repairs\":[{}],\"information\":[{}],\"failures\":[{}]}}",
        pages.map_or_else(|| "null".into(), |count| count.to_string()),
        repair_tags(context.document()),
        information_tags(context.document()),
        failures.join(",")
    )
}

fn scan_open_error(error: &(dyn Error + 'static)) -> String {
    format!(
        "{{\"schema\":1,\"strict\":false,\"open\":{},\"pages\":null}}",
        json_error(error)
    )
}

/// Pagura-only stage timings for profiling; never a cross-engine claim.
fn microbench(input: &str, operation: &str, count: u32, dpi: f64, flags: &Flags) -> CliResult {
    if !(1..=100_000).contains(&count) {
        return Err("iteration count must be 1 to 100000".into());
    }
    let options = OpenOptions::default();
    let bytes = load(input, options.limits)?;
    let render_options = flags.render_options(dpi);
    let warm = pagura::render_context(std::sync::Arc::new(Document::open(
        bytes.clone(),
        &options,
    )?));
    let started = Instant::now();
    for _ in 0..count {
        match operation {
            "open-pages" => {
                let document = Document::open(bytes.clone(), &options)?;
                black_box(document.page_count()?);
            }
            "render-cold" => {
                let document = Document::open(bytes.clone(), &options)?;
                black_box(pagura::render_page(
                    &document,
                    document.page(0)?,
                    &render_options,
                )?);
            }
            "render-warm" => {
                black_box(warm.render_page(warm.document().page(0)?, &render_options)?);
            }
            _ => return Err("unknown microbenchmark operation".into()),
        }
    }
    println!(
        "{{\"schema\":2,\"operation\":\"{operation}\",\"iterations\":{count},\"elapsed_ns\":{}}}",
        started.elapsed().as_nanos()
    );
    Ok(())
}
