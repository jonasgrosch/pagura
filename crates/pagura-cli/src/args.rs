//! Command-line parsing: the arguments become a typed [`Command`], or a
//! [`UsageError`] that exits with status 2 before any input is read.

use pagura::{OverprintPreview, Recovery, RenderOptions, Rgba8};
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::ops::RangeInclusive;
use std::path::PathBuf;
use std::str::FromStr;

/// The synopsis printed with every usage error.
pub(crate) const USAGE: &str = "\
usage: pagura inspect [--strict] [--password TEXT] INPUT
       pagura metadata [--strict] [--password TEXT] INPUT
       pagura attachments [--strict] [--password TEXT] INPUT
       pagura extract-metadata [--strict] [--password TEXT] INPUT OUTPUT
       pagura extract-attachment [--strict] [--password TEXT] INPUT INDEX KEY OUTPUT
       pagura text [--strict] [--password TEXT] INPUT [PAGE]
       pagura find [--strict] [--password TEXT] INPUT QUERY
       pagura render [--strict] [--password TEXT] [--overprint MODE] [--timings]
              [--format ppm|png] [--transparent] INPUT OUTPUT [DPI [PAGE]]
       pagura convert [--strict] [--password TEXT] [--overprint MODE] [--timings]
              [--format ppm|png] [--transparent] INPUT OUTPUT_DIR [DPI [FIRST LAST]]
       pagura scan [--password TEXT] [--overprint MODE] [--max-pages N] INPUT [DPI]
       pagura roundtrip [--password TEXT] INPUT OUTPUT
       pagura microbench [--strict] [--password TEXT] [--overprint MODE]
              INPUT OPERATION COUNT [DPI]
       pagura licenses
       pagura [--overprint MODE] --version
       pagura --help";

/// The rest of the help text, printed by `--help` after [`USAGE`].
pub(crate) const HELP: &str = "\
commands:
  inspect     print one JSON line: page/object counts, page geometry, size,
              diagnostics
  metadata    print Info fields, document language and metadata stream identity
              as JSON; Info and XMP remain separate sources
  attachments print catalog EmbeddedFiles entries and EF payload keys as JSON;
              Unicode names and original path/key bytes are retained separately
  extract-metadata
              decode the catalog metadata stream to the explicit OUTPUT path
  extract-attachment
              decode EF payload KEY (F, UF, DOS, Mac or Unix) from catalog
              EmbeddedFiles INDEX (starting at 0) to the explicit OUTPUT path
  text        print the text of every page, or of PAGE, separated by form feeds
  find        print one JSON line per match: page, character range and quads
              in default user space
  render      render PAGE (default 0) to PPM (default) or PNG over white
  convert     render every page, or FIRST to LAST, to OUTPUT_DIR/page-NNNN.FORMAT
  scan        open and render every page without writing images, and print
              one JSON line describing what opened, rendered and failed; it
              opens strictly first and repairs only when that fails
  roundtrip   open strictly and save the original bytes unchanged
  microbench  time COUNT (1 to 100000) repetitions of OPERATION on page 0 of
              a document opened once: open-pages opens it again;
              render-cold records and renders with a new context, so no
              font, image or scene is reused; record-cold records with a new
              context and keeps no scene; record-warm records with one
              context, which reuses fonts, glyphs and images but keeps no
              scene; render-warm renders from one context, which records the
              page once and keeps its scene. Pagura-only profiling, never a
              cross-engine comparison
  licenses    print the notices of the data linked into this build

PAGE, FIRST and LAST count from 0; convert names its images from 1
(page-0001.ppm is page 0), as pdftoppm does. DPI is a positive number,
72 by default. Outputs are created and never overwrite a file.

options:
  --password TEXT  password for encrypted documents (default: empty)
  --strict         reject malformed input instead of repairing it
  --overprint MODE simulate overprinting (8.6.7): auto (the default) where
                   the document declares a PDF/X print intent, always, or
                   never
  --timings        print JSON stage timings to stdout: open (read and parse
                   the file, build the page list), render (interpret and
                   rasterize) and emit (encode and write the image); the
                   engine's contribution to a conversion is open + render
  --format FORMAT  render/convert output: ppm (default) or png
  --transparent    transparent PNG background; requires --format png
  --max-pages N    scan at most N pages per document
  --version        print the version
  --help, -h       print this help

An option that a command does not take is a usage error; --version accepts
the rendering option --overprint and ignores it, so a harness can put it
before every invocation. Exit status: 0 on success, 1
when the command fails, 2 for a usage error.
";

/// Microbenchmark iterations accepted, inclusive.
const BENCH_COUNTS: RangeInclusive<u32> = 1..=100_000;

/// What to do.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Command {
    /// Print [`USAGE`] and [`HELP`].
    Help,
    /// Print the version.
    Version,
    /// Print the licence notices of the linked data.
    Licenses,
    /// Print a JSON summary of a document.
    Inspect(Source),
    /// Print typed document metadata as JSON.
    Metadata(Source),
    /// Print catalog attachment entries as JSON, without decoding their payloads.
    Attachments(Source),
    /// Decode the document metadata stream to a new, caller-named file.
    ExtractMetadata { source: Source, output: PathBuf },
    /// Decode a selected catalog attachment payload to a new, caller-named file.
    ExtractAttachment {
        source: Source,
        index: usize,
        key: String,
        output: PathBuf,
    },
    /// Print the text of every page, or of one zero-based page.
    Text { source: Source, page: Option<usize> },
    /// Print the matches of a query as JSON lines.
    Find { source: Source, query: String },
    /// Render pages to PPM or PNG images: `render` and `convert`.
    Convert(Conversion),
    /// Open and render without writing images; print a JSON report.
    Scan {
        source: Source,
        render: RenderOptions,
        max_pages: Option<usize>,
    },
    /// Open strictly and save the original bytes.
    Roundtrip { source: Source, output: PathBuf },
    /// Time repeated engine work.
    Microbench {
        source: Source,
        operation: Bench,
        count: u32,
        render: RenderOptions,
    },
}

/// An input document and how to open it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Source {
    pub(crate) path: PathBuf,
    pub(crate) password: Vec<u8>,
    pub(crate) recovery: Recovery,
}

/// A `render` or `convert` request.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Conversion {
    pub(crate) source: Source,
    pub(crate) output: Output,
    pub(crate) format: ImageFormat,
    /// The resolution and overprint policy.
    pub(crate) render: RenderOptions,
    /// Zero-based and inclusive; all pages when absent.
    pub(crate) pages: Option<RangeInclusive<usize>>,
    pub(crate) timings: Timings,
}

/// Image encodings exposed by render/convert; selection is explicit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum ImageFormat {
    #[default]
    Ppm,
    Png,
}

impl ImageFormat {
    pub(crate) const fn extension(self) -> &'static str {
        match self {
            Self::Ppm => "ppm",
            Self::Png => "png",
        }
    }
}

/// Where converted pages go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Output {
    /// One page to one file.
    File(PathBuf),
    /// Every page to `page-NNNN.FORMAT` in an existing directory.
    Directory(PathBuf),
}

/// Whether a conversion prints its stage timings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Timings {
    Off,
    Print,
}

/// A microbenchmark operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Bench {
    /// Open the document and build its page list.
    OpenPages,
    /// Record and render the first page with a new render context.
    RenderCold,
    /// Record the first page with a new render context, keeping no scene.
    RecordCold,
    /// Record the first page with one render context, keeping no scene.
    RecordWarm,
    /// Render the first page from one render context, which keeps its scene.
    RenderWarm,
}

impl Bench {
    const ALL: [Self; 5] = [
        Self::OpenPages,
        Self::RenderCold,
        Self::RecordCold,
        Self::RecordWarm,
        Self::RenderWarm,
    ];

    /// The operation's command-line and JSON name.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::OpenPages => "open-pages",
            Self::RenderCold => "render-cold",
            Self::RecordCold => "record-cold",
            Self::RecordWarm => "record-warm",
            Self::RenderWarm => "render-warm",
        }
    }
}

/// Arguments that do not form a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UsageError(String);

impl fmt::Display for UsageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for UsageError {}

fn usage(message: impl Into<String>) -> UsageError {
    UsageError(message.into())
}

/// An option, which may appear anywhere on the command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flag {
    Strict,
    Timings,
    Password,
    Overprint,
    MaxPages,
    Format,
    Transparent,
    Version,
}

impl Flag {
    const ALL: [Self; 8] = [
        Self::Strict,
        Self::Timings,
        Self::Password,
        Self::Overprint,
        Self::MaxPages,
        Self::Format,
        Self::Transparent,
        Self::Version,
    ];

    const fn name(self) -> &'static str {
        match self {
            Self::Strict => "--strict",
            Self::Timings => "--timings",
            Self::Password => "--password",
            Self::Overprint => "--overprint",
            Self::MaxPages => "--max-pages",
            Self::Format => "--format",
            Self::Transparent => "--transparent",
            Self::Version => "--version",
        }
    }
}

/// The options given, before the command decides which apply.
#[derive(Debug, Default)]
struct Options {
    given: Vec<Flag>,
    password: Vec<u8>,
    overprint: OverprintPreview,
    max_pages: Option<usize>,
    format: ImageFormat,
}

impl Options {
    fn has(&self, flag: Flag) -> bool {
        self.given.contains(&flag)
    }

    fn take(
        &mut self,
        flag: Flag,
        args: &mut impl Iterator<Item = OsString>,
    ) -> Result<(), UsageError> {
        let mut value = || {
            args.next()
                .ok_or_else(|| usage(format!("{} needs a value", flag.name())))
        };
        match flag {
            Flag::Password => {
                let password = value()?;
                let text = password
                    .to_str()
                    .ok_or_else(|| usage("--password must be UTF-8 text"))?;
                self.password = text.as_bytes().to_vec();
            }
            Flag::Overprint => {
                self.overprint = match value()?.to_str() {
                    Some("auto") => OverprintPreview::Auto,
                    Some("always") => OverprintPreview::Always,
                    Some("never") => OverprintPreview::Never,
                    _ => return Err(usage("--overprint must be auto, always or never")),
                };
            }
            Flag::MaxPages => self.max_pages = Some(number(&value()?, "--max-pages")?),
            Flag::Format => {
                self.format = match value()?.to_str() {
                    Some("ppm") => ImageFormat::Ppm,
                    Some("png") => ImageFormat::Png,
                    _ => return Err(usage("--format must be ppm or png")),
                };
            }
            Flag::Strict | Flag::Timings | Flag::Transparent | Flag::Version => {}
        }
        self.given.push(flag);
        Ok(())
    }

    /// Rejects every given option that `command` does not take.
    fn allow(&self, command: &str, allowed: &[Flag]) -> Result<(), UsageError> {
        match self.given.iter().find(|flag| !allowed.contains(flag)) {
            Some(flag) => Err(usage(format!(
                "{} does not apply to {command}",
                flag.name()
            ))),
            None => Ok(()),
        }
    }

    fn source(&self, path: &OsStr, recovery: Recovery) -> Source {
        Source {
            path: PathBuf::from(path),
            password: self.password.clone(),
            recovery,
        }
    }

    /// The source of a command that takes `--strict`.
    fn chosen_source(&self, path: &OsStr) -> Source {
        let recovery = if self.has(Flag::Strict) {
            Recovery::Strict
        } else {
            Recovery::Repair
        };
        self.source(path, recovery)
    }

    /// Rendering at the optional DPI argument with the overprint policy.
    fn render(&self, dpi: Option<&OsString>) -> Result<RenderOptions, UsageError> {
        Ok(RenderOptions {
            overprint: self.overprint,
            background: if self.has(Flag::Transparent) {
                Rgba8::TRANSPARENT
            } else {
                Rgba8::WHITE
            },
            ..RenderOptions::from_dpi(self::dpi(dpi)?)
        })
    }

    fn image_format(&self) -> Result<ImageFormat, UsageError> {
        if self.has(Flag::Transparent) && self.format != ImageFormat::Png {
            return Err(usage("--transparent requires --format png"));
        }
        Ok(self.format)
    }

    fn timings(&self) -> Timings {
        if self.has(Flag::Timings) {
            Timings::Print
        } else {
            Timings::Off
        }
    }
}

/// Parses the arguments after the program name.
///
/// # Errors
/// A [`UsageError`] naming the first argument that does not fit.
pub(crate) fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Command, UsageError> {
    let mut options = Options::default();
    let mut positional = Vec::new();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--help" | "-h") => return Ok(Command::Help),
            Some(option) if option.starts_with("--") => {
                let flag = Flag::ALL
                    .into_iter()
                    .find(|flag| flag.name() == option)
                    .ok_or_else(|| usage(format!("unknown option {option}")))?;
                options.take(flag, &mut args)?;
            }
            _ => positional.push(arg),
        }
    }
    let Some((name, operands)) = positional.split_first() else {
        if options.has(Flag::Version) {
            // Rendering options are accepted and ignored, so a harness can
            // put them before every invocation, its version check included.
            let allowed = [Flag::Version, Flag::Overprint];
            options.allow("--version", &allowed)?;
            return Ok(Command::Version);
        }
        return Err(usage("missing command"));
    };
    let name = name
        .to_str()
        .ok_or_else(|| usage(format!("unknown command {}", name.to_string_lossy())))?;
    command(name, operands, &options)
}

/// The command `name` with its `operands`, if it takes the given options.
fn command(name: &str, operands: &[OsString], options: &Options) -> Result<Command, UsageError> {
    use Flag::{MaxPages, Overprint, Password, Strict};
    if let Some(command) = document_command(name, operands, options)? {
        return Ok(command);
    }
    if let Some(command) = image_command(name, operands, options)? {
        return Ok(command);
    }
    let command = match (name, operands) {
        ("licenses", []) => {
            options.allow(name, &[])?;
            Command::Licenses
        }
        ("inspect", [input]) => {
            options.allow(name, &[Strict, Password])?;
            Command::Inspect(options.chosen_source(input))
        }
        ("text", [input, page @ ..]) if page.len() <= 1 => {
            options.allow(name, &[Strict, Password])?;
            Command::Text {
                source: options.chosen_source(input),
                page: page.first().map(|page| number(page, "PAGE")).transpose()?,
            }
        }
        ("find", [input, query]) => {
            options.allow(name, &[Strict, Password])?;
            Command::Find {
                source: options.chosen_source(input),
                query: text(query, "QUERY")?.to_owned(),
            }
        }
        ("scan", [input, rest @ ..]) if rest.len() <= 1 => {
            options.allow(name, &[Password, Overprint, MaxPages])?;
            Command::Scan {
                source: options.source(input, Recovery::Strict),
                render: options.render(rest.first())?,
                max_pages: options.max_pages,
            }
        }
        ("roundtrip", [input, output]) => {
            options.allow(name, &[Password])?;
            Command::Roundtrip {
                source: options.source(input, Recovery::Strict),
                output: PathBuf::from(output),
            }
        }
        ("microbench", [input, operation, count, rest @ ..]) if rest.len() <= 1 => {
            options.allow(name, &[Strict, Password, Overprint])?;
            let operation = Bench::ALL
                .into_iter()
                .find(|bench| operation.to_str() == Some(bench.name()))
                .ok_or_else(|| {
                    usage(
                        "OPERATION must be open-pages, render-cold, record-cold, record-warm or render-warm",
                    )
                })?;
            let count: u32 = number(count, "COUNT")?;
            if !BENCH_COUNTS.contains(&count) {
                return Err(usage("COUNT must be 1 to 100000"));
            }
            Command::Microbench {
                source: options.chosen_source(input),
                operation,
                count,
                render: options.render(rest.first())?,
            }
        }
        (
            "licenses" | "inspect" | "text" | "find" | "render" | "convert" | "scan" | "roundtrip"
            | "microbench",
            _,
        ) => return Err(usage(format!("wrong number of arguments for {name}"))),
        _ => return Err(usage(format!("unknown command {name}"))),
    };
    Ok(command)
}

/// Output format and background options apply only to image conversion commands.
fn image_command(
    name: &str,
    operands: &[OsString],
    options: &Options,
) -> Result<Option<Command>, UsageError> {
    if !matches!(name, "render" | "convert") {
        return Ok(None);
    }
    use Flag::{Format, Overprint, Password, Strict, Timings, Transparent};
    options.allow(
        name,
        &[Strict, Password, Overprint, Timings, Format, Transparent],
    )?;
    let command = match (name, operands) {
        ("render", [input, output, rest @ ..]) if rest.len() <= 2 => {
            let page = rest.get(1).map_or(Ok(0), |page| number(page, "PAGE"))?;
            Command::Convert(Conversion {
                source: options.chosen_source(input),
                output: Output::File(PathBuf::from(output)),
                format: options.image_format()?,
                render: options.render(rest.first())?,
                pages: Some(page..=page),
                timings: options.timings(),
            })
        }
        ("convert", [input, output, rest @ ..]) if matches!(rest.len(), 0 | 1 | 3) => {
            let pages = match rest {
                [_, first, last] => Some(number(first, "FIRST")?..=number(last, "LAST")?),
                _ => None,
            };
            Command::Convert(Conversion {
                source: options.chosen_source(input),
                output: Output::Directory(PathBuf::from(output)),
                format: options.image_format()?,
                render: options.render(rest.first())?,
                pages,
                timings: options.timings(),
            })
        }
        _ => return Err(usage(format!("wrong number of arguments for {name}"))),
    };
    Ok(Some(command))
}

fn document_command(
    name: &str,
    operands: &[OsString],
    options: &Options,
) -> Result<Option<Command>, UsageError> {
    if !matches!(
        name,
        "metadata" | "attachments" | "extract-metadata" | "extract-attachment"
    ) {
        return Ok(None);
    }
    options.allow(name, &[Flag::Strict, Flag::Password])?;
    let command = match (name, operands) {
        ("metadata", [input]) => Command::Metadata(options.chosen_source(input)),
        ("attachments", [input]) => Command::Attachments(options.chosen_source(input)),
        ("extract-metadata", [input, output]) => Command::ExtractMetadata {
            source: options.chosen_source(input),
            output: PathBuf::from(output),
        },
        ("extract-attachment", [input, index, key, output]) => Command::ExtractAttachment {
            source: options.chosen_source(input),
            index: number(index, "INDEX")?,
            key: text(key, "KEY")?.to_owned(),
            output: PathBuf::from(output),
        },
        _ => return Err(usage(format!("wrong number of arguments for {name}"))),
    };
    Ok(Some(command))
}

fn text<'a>(value: &'a OsStr, what: &str) -> Result<&'a str, UsageError> {
    value
        .to_str()
        .ok_or_else(|| usage(format!("{what} must be UTF-8 text")))
}

/// A non-negative integer argument; the error names the argument.
fn number<T: FromStr>(value: &OsStr, what: &str) -> Result<T, UsageError> {
    text(value, what)?.parse().map_err(|_| {
        usage(format!(
            "{what} must be a non-negative integer, not {}",
            value.to_string_lossy()
        ))
    })
}

/// The optional DPI argument: a positive finite number, 72 when absent.
fn dpi(value: Option<&OsString>) -> Result<f64, UsageError> {
    let Some(value) = value else {
        return Ok(72.0);
    };
    text(value, "DPI")?
        .parse::<f64>()
        .ok()
        .filter(|dpi| dpi.is_finite() && *dpi > 0.0)
        .ok_or_else(|| {
            usage(format!(
                "DPI must be a positive number, not {}",
                value.to_string_lossy()
            ))
        })
}

#[cfg(test)]
mod tests;
