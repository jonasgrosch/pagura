//! Real documents: the benchmark pages rendered by the renderer (Exact,
//! through `pagura-render` as the CLI renders them) and by the reference,
//! against the previous renderer's goldens (ADR 0004 §3).
//!
//! Environment: `PAGURA_REFERENCE_BENCH` (the page list, JSON entries with
//! `file` and `page`), `PAGURA_REFERENCE_CORPORA` (the corpus root the
//! files are relative to), `PAGURA_REFERENCE_BENCH_GOLDENS` (the exact
//! goldens, `<dpi>/<stem>-p<page>.ppm.gz`), `PAGURA_REFERENCE_DPI` (comma
//! separated, default 72), `PAGURA_REFERENCE_THREADS` (default 4) and
//! `PAGURA_REFERENCE_REPORT` (the directory of `bench.json`).

use crate::compare::rgb_on_white;
use crate::goldens::load_path;
use pagura_document::{Document, OpenOptions};
use pagura_raster::quantize_unit;
use pagura_reference::{Overprint, Quantization, Request};
use pagura_render::RenderOptions;
use pagura_testkit::Json;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// One page at one resolution.
#[derive(Debug, Clone)]
struct Item {
    file: String,
    page: usize,
    dpi: u32,
}

/// What one page gave.
#[derive(Debug, Default)]
struct Outcome {
    pixels: usize,
    /// Pixels with a channel more than one level from the reference.
    beyond: usize,
    /// Raw-area diagnostic for those pixels. The contract's acceptance
    /// gate uses `beyond`: scene composition references coverage bytes.
    beyond_both: usize,
    /// Channels equal to the correctly rounded value: new and golden.
    new_rounded: usize,
    golden_rounded: usize,
    channels: usize,
    /// Channels that changed against the golden.
    changed: usize,
    changed_pixels: usize,
    /// Of those, within one level of the reference: golden and new.
    golden_within: usize,
    new_within: usize,
    /// Of those, correctly rounded: golden and new.
    golden_changed_rounded: usize,
    new_changed_rounded: usize,
    /// The largest distance from the reference, in levels: new and golden.
    new_max: f64,
    golden_max: f64,
    /// Seconds the reference took.
    seconds: f64,
    /// The first pixels beyond one level: x, y, channel, new byte, exact.
    first_beyond: Vec<(u32, u32, usize, u8, f64)>,
    error: Option<String>,
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// Runs the comparison over the page list; returns false when nothing was
/// configured.
pub fn run() -> bool {
    let (Some(list), Some(corpora), Some(goldens)) = (
        env("PAGURA_REFERENCE_BENCH"),
        env("PAGURA_REFERENCE_CORPORA"),
        env("PAGURA_REFERENCE_BENCH_GOLDENS"),
    ) else {
        return false;
    };
    let dpis: Vec<u32> = env("PAGURA_REFERENCE_DPI")
        .unwrap_or_else(|| "72".to_owned())
        .split(',')
        .filter_map(|d| d.trim().parse().ok())
        .collect();
    let threads: usize = env("PAGURA_REFERENCE_THREADS")
        .and_then(|t| t.parse().ok())
        .unwrap_or(4);
    let text = std::fs::read_to_string(&list).unwrap_or_default();
    let pages = Json::parse(&text).ok();
    let mut items = Vec::new();
    for &dpi in &dpis {
        for entry in pages.as_ref().and_then(Json::as_array).unwrap_or_default() {
            if let (Some(file), Some(page)) = (
                entry.get("file").and_then(Json::as_str),
                entry.get("page").and_then(Json::as_u64),
            ) {
                items.push(Item {
                    file: file.to_owned(),
                    page: usize::try_from(page).unwrap_or(usize::MAX),
                    dpi,
                });
            }
        }
    }
    let queue = Mutex::new(items.into_iter().enumerate().collect::<Vec<_>>());
    let results = Mutex::new(Vec::new());
    let provider = pagura_data::Bundled::default();
    std::thread::scope(|scope| {
        for _ in 0..threads.max(1) {
            scope.spawn(|| {
                loop {
                    let next = queue.lock().ok().and_then(|mut q| q.pop());
                    let Some((index, item)) = next else {
                        break;
                    };
                    let outcome =
                        compare(&item, Path::new(&corpora), Path::new(&goldens), &provider);
                    println!("{}", line(&item, &outcome));
                    if let Ok(mut results) = results.lock() {
                        results.push((index, item, outcome));
                    }
                }
            });
        }
    });
    let mut results = results.into_inner().unwrap_or_default();
    results.sort_by_key(|(index, ..)| *index);
    write(&results);
    let failed = results
        .iter()
        .filter(|(_, _, outcome)| outcome.failed())
        .count();
    assert_eq!(
        failed, 0,
        "benchmark reference failures; see the per-page report for errors and pixels beyond one level"
    );
    true
}

fn stem(item: &Item) -> String {
    let name = Path::new(&item.file)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    format!("{name}-p{}", item.page)
}

fn compare(
    item: &Item,
    corpora: &Path,
    goldens: &Path,
    provider: &pagura_data::Bundled,
) -> Outcome {
    let mut outcome = Outcome::default();
    if let Err(error) = measure(item, corpora, goldens, provider, &mut outcome) {
        outcome.error = Some(error);
    }
    outcome
}

fn measure(
    item: &Item,
    corpora: &Path,
    goldens: &Path,
    provider: &pagura_data::Bundled,
    outcome: &mut Outcome,
) -> Result<(), String> {
    let describe = |e: pagura_core::Error| crate::report::describe(&e);
    let bytes = std::fs::read(corpora.join(&item.file)).map_err(|e| e.to_string())?;
    let document = Document::open(bytes, &OpenOptions::default()).map_err(describe)?;
    let page = document.page(item.page).map_err(describe)?;
    let options = RenderOptions::from_dpi(f64::from(item.dpi));
    let rendered =
        pagura_render::render_page(&document, page, &options, Some(provider)).map_err(describe)?;
    let scene = pagura_render::record_page(&document, page, Some(provider)).map_err(describe)?;
    let (width, height) = (rendered.width(), rendered.height());
    let golden = previous(item, goldens, (width, height))?;
    let scale = f64::from(item.dpi) / 72.0;
    let region = pagura_raster::DeviceRect {
        x0: 0,
        y0: 0,
        x1: width,
        y1: height,
    };
    let device = pagura_paint::RenderRequest::page(&scene.page, scale).map_err(describe)?;
    let mut request = Request::new(device.transform, region);
    request.overprint = if scene.print_intent {
        Overprint::Simulate
    } else {
        Overprint::Ignore
    };
    let start = std::time::Instant::now();
    let reference =
        pagura_reference::render(&scene, &request, &document.limits()).map_err(describe)?;
    outcome.seconds = start.elapsed().as_secs_f64();
    request.quantization = Quantization::Exact;
    let unquantized =
        pagura_reference::render(&scene, &request, &document.limits()).map_err(describe)?;
    let new = rgb_on_white(rendered.data());
    let mut changed_pixels = pixel_report(item)?;
    let coordinates = (0..height).flat_map(|y| (0..width).map(move |x| (x, y)));
    for (((n, g), exact), (x, y)) in new
        .as_chunks::<3>()
        .0
        .iter()
        .zip(golden.as_chunks::<3>().0)
        .zip(reference.values().iter().zip(unquantized.values()))
        .zip(coordinates)
    {
        let (exact, area) = exact;
        outcome.pixels += 1;
        let mut beyond = false;
        let mut beyond_area = false;
        let mut changed = false;
        for channel in 0..3 {
            let value = exact[channel] + (1.0 - exact[3]);
            let level = value * 255.0;
            let rounded = quantize_unit(value);
            let (new_error, golden_error) = (
                (f64::from(n[channel]) - level).abs(),
                (f64::from(g[channel]) - level).abs(),
            );
            outcome.channels += 1;
            outcome.new_rounded += usize::from(n[channel] == rounded);
            outcome.golden_rounded += usize::from(g[channel] == rounded);
            outcome.new_max = outcome.new_max.max(new_error);
            outcome.golden_max = outcome.golden_max.max(golden_error);
            let area_level = (area[channel] + (1.0 - area[3])) * 255.0;
            if new_error > 1.0 + 1e-6 {
                beyond = true;
                if outcome.first_beyond.len() < 8 {
                    outcome
                        .first_beyond
                        .push((x, y, channel, n[channel], level));
                }
                if (f64::from(n[channel]) - area_level).abs() > 1.0 + 1e-6 {
                    beyond_area = true;
                }
            }
            if n[channel] != g[channel] {
                changed = true;
                outcome.changed += 1;
                outcome.golden_within += usize::from(golden_error <= 1.0 + 1e-6);
                outcome.new_within += usize::from(new_error <= 1.0 + 1e-6);
                outcome.golden_changed_rounded += usize::from(g[channel] == rounded);
                outcome.new_changed_rounded += usize::from(n[channel] == rounded);
                if let Some(report) = &mut changed_pixels {
                    write_pixel(
                        report,
                        (x, y),
                        channel,
                        [g[channel], n[channel]],
                        [level, area_level],
                    )?;
                }
            }
        }
        outcome.beyond += usize::from(beyond);
        outcome.beyond_both += usize::from(beyond_area);
        outcome.changed_pixels += usize::from(changed);
    }
    Ok(())
}

fn previous(item: &Item, goldens: &Path, (width, height): (u32, u32)) -> Result<Vec<u8>, String> {
    let path = goldens
        .join(item.dpi.to_string())
        .join(format!("{}.ppm.gz", stem(item)));
    let (old_width, old_height, pixels) = load_path(&path).ok_or("no golden")?;
    if (old_width, old_height) != (width, height) {
        return Err(format!(
            "golden is {old_width}×{old_height}, render {width}×{height}"
        ));
    }
    Ok(pixels)
}

fn write_pixel(
    report: &mut std::io::BufWriter<std::fs::File>,
    (x, y): (u32, u32),
    channel: usize,
    [old, new]: [u8; 2],
    [reference, area]: [f64; 2],
) -> Result<(), String> {
    use std::io::Write;
    writeln!(
        report,
        "{x},{y},{channel},{old},{new},{reference:.12},{area:.12}"
    )
    .map_err(|e| e.to_string())
}

/// Optional per-channel evidence for ADR 0004 section 3. Stored separately
/// from the compact summary so large pages need no retained pixel list.
fn pixel_report(item: &Item) -> Result<Option<std::io::BufWriter<std::fs::File>>, String> {
    use std::io::Write;
    let Some(directory) = env("PAGURA_REFERENCE_PIXELS") else {
        return Ok(None);
    };
    let directory = PathBuf::from(directory).join(item.dpi.to_string());
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let file = std::fs::File::create(directory.join(format!("{}.csv", stem(item))))
        .map_err(|e| e.to_string())?;
    let mut report = std::io::BufWriter::new(file);
    writeln!(
        report,
        "x,y,channel,previous,new,reference_levels,unquantized_levels"
    )
    .map_err(|e| e.to_string())?;
    Ok(Some(report))
}

fn rate(part: usize, whole: usize) -> f64 {
    if whole == 0 {
        1.0
    } else {
        part as f64 / whole as f64
    }
}

fn line(item: &Item, o: &Outcome) -> String {
    match &o.error {
        Some(error) => format!("error     {} @{} ({error})", stem(item), item.dpi),
        None => format!(
            "{} {} @{}: {} pixels, beyond {} (also with exact areas {}), changed {} channels (golden within {}, new within {}; \
             correctly rounded golden {}, new {}), exact rounding golden {:.6} new {:.6}, max error \
             golden {:.3} new {:.3}, reference {:.1} s{}",
            if o.beyond > 0 {
                "BEYOND   "
            } else {
                "page     "
            },
            stem(item),
            item.dpi,
            o.pixels,
            o.beyond,
            o.beyond_both,
            o.changed,
            o.golden_within,
            o.new_within,
            o.golden_changed_rounded,
            o.new_changed_rounded,
            rate(o.golden_rounded, o.channels),
            rate(o.new_rounded, o.channels),
            o.golden_max,
            o.new_max,
            o.seconds,
            if o.first_beyond.is_empty() {
                String::new()
            } else {
                format!(" first beyond {:?}", o.first_beyond)
            },
        ),
    }
}

impl Outcome {
    fn failed(&self) -> bool {
        self.error.is_some() || self.beyond != 0
    }
}

#[test]
fn raw_area_diagnostics_cannot_excuse_a_coverage_byte_reference_failure() {
    let outcome = Outcome {
        beyond: 1,
        beyond_both: 0,
        ..Outcome::default()
    };
    assert!(outcome.failed());
    assert!(!Outcome::default().failed());
}

fn write(results: &[(usize, Item, Outcome)]) {
    let mut json = String::from("{\"section\":\"bench\",\"pages\":[");
    for (i, (_, item, o)) in results.iter().enumerate() {
        if i > 0 {
            json.push(',');
        }
        let _ = write!(
            json,
            "{{\"file\":{:?},\"page\":{},\"dpi\":{},\"error\":{},\"pixels\":{},\"beyond\":{},\"beyond_with_exact_areas\":{},\"channels\":{},\
             \"changed_channels\":{},\"changed_pixels\":{},\"golden_within_one\":{},\"new_within_one\":{},\
             \"golden_changed_correctly_rounded\":{},\"new_changed_correctly_rounded\":{},\
             \"golden_correctly_rounded\":{},\"new_correctly_rounded\":{},\"golden_max_error\":{},\
             \"new_max_error\":{},\"reference_seconds\":{}}}",
            item.file,
            item.page,
            item.dpi,
            o.error
                .as_ref()
                .map_or_else(|| "null".to_owned(), |e| format!("{e:?}")),
            o.pixels,
            o.beyond,
            o.beyond_both,
            o.channels,
            o.changed,
            o.changed_pixels,
            o.golden_within,
            o.new_within,
            o.golden_changed_rounded,
            o.new_changed_rounded,
            o.golden_rounded,
            o.new_rounded,
            o.golden_max,
            o.new_max,
            o.seconds,
        );
    }
    json.push_str("]}\n");
    if let Some(directory) = std::env::var_os("PAGURA_REFERENCE_REPORT") {
        let directory = PathBuf::from(directory);
        let _ = std::fs::create_dir_all(&directory);
        let _ = std::fs::write(directory.join("bench.json"), json);
    }
}
