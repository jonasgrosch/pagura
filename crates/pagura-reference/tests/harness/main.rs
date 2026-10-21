//! The renderer (`pagura-paint`) against the reference over every render
//! case of the conformance fixtures (ADR 0004 §2–§3, ADR 0034):
//!
//! - the renderer is within one level of the reference at every pixel;
//!   each case reports its pixels beyond one level, its changed pixels with
//!   their exact values and its exact-rounding match rate, and with
//!   `PAGURA_REFERENCE_GOLDENS` also its changes against the previous
//!   renderer's goldens;
//! - the reference meets the fixtures' independent oracles;
//! - pixels, errors (kind, object, position) and reserved charges do not
//!   depend on band height, region partition or whether store values are
//!   cached;
//! - each fast parameter alone, reported against the reference, never
//!   failing.
//!
//! A case whose recording or rendering meets a dependency's placeholder is
//! reported as pending. Run with `-- --nocapture` for one line per case;
//! `PAGURA_REFERENCE_REPORT=<dir>` writes a JSON summary per test there,
//! `PAGURA_REFERENCE_FILTER=<text>` keeps the cases whose id contains it,
//! and `PAGURA_REFERENCE_FULL=1` probes charges on every case rather than
//! on every eighth.

mod cases;
mod compare;
mod goldens;
mod renderer;
mod report;

use cases::{Case, Recorded, pending, render_cases};
use compare::{Comparison, parse_ppm, rgb_on_white};
use pagura_core::{Error, ErrorKind, Limits};
use pagura_paint::FastParams;
use pagura_raster::DeviceRect;
use renderer::{Cache, Run, fresh};
use report::{Report, Status, describe};

/// Band heights every case renders at.
const BAND_HEIGHTS: [u32; 5] = [1, 7, 16, 32, 256];

/// What recording a case gave.
enum Recording {
    Ready(Box<Recorded>),
    Done(Status),
}

fn record(case: &Case, provider: &dyn pagura_text::Provider) -> Recording {
    match case.record(provider) {
        Ok(recorded) => Recording::Ready(Box::new(recorded)),
        Err(error) if pending(&error) => Recording::Done(Status::Pending(describe(&error))),
        Err(error) => Recording::Done(Status::Declined(format!("recording: {}", describe(&error)))),
    }
}

/// The status of a case whose renderer or reference failed.
fn failed(rendered: Option<&Error>, reference: Option<&Error>) -> Status {
    if let Some(error) = rendered.into_iter().chain(reference).find(|e| pending(e)) {
        return Status::Pending(describe(error));
    }
    let side =
        |name: &str, error: Option<&Error>| error.map(|e| format!("{name}: {}", describe(e)));
    let reasons: Vec<String> = [side("renderer", rendered), side("reference", reference)]
        .into_iter()
        .flatten()
        .collect();
    Status::Declined(reasons.join("; "))
}

#[test]
fn the_renderer_is_within_one_level_of_the_reference() {
    let provider = pagura_data::Bundled::default();
    let goldens = goldens::directory();
    let (mut report, mut changes) = (Report::default(), Report::default());
    for case in render_cases() {
        let recorded = match record(&case, &provider) {
            Recording::Ready(recorded) => recorded,
            Recording::Done(status) => {
                report.push(&case.id, status);
                continue;
            }
        };
        let run = Run::page(&recorded);
        let (rendered, reference) = (run.render(&recorded), run.reference(&recorded));
        let (rendered, reference) = match (rendered, reference) {
            (Ok(rendered), Ok(reference)) => (rendered, reference),
            (rendered, reference) => {
                report.push(
                    &case.id,
                    failed(rendered.as_ref().err(), reference.as_ref().err()),
                );
                continue;
            }
        };
        report.push(
            &case.id,
            Status::Compared(Comparison::of(&rendered, &reference)),
        );
        if let Some(directory) = &goldens {
            let status = match goldens::load(directory, &case.id) {
                Some((width, height, golden))
                    if (width, height)
                        == (reference.region().width(), reference.region().height()) =>
                {
                    Status::Changed(goldens::Changes::of(&rendered, &golden, &reference))
                }
                _ => Status::Declined("no golden of the page's size".to_owned()),
            };
            changes.push(&case.id, status);
        }
    }
    report.finish("differential");
    if goldens.is_some() {
        changes.finish("goldens");
    }
    let failures: Vec<_> = report.failures().iter().map(|e| e.id.clone()).collect();
    assert!(
        failures.is_empty(),
        "beyond one level of the reference: {failures:?}"
    );
}

#[test]
fn the_reference_meets_the_fixture_oracles() {
    let provider = pagura_data::Bundled::default();
    let mut report = Report::default();
    for case in render_cases() {
        let Some(oracle) = case
            .oracle
            .as_ref()
            .and_then(|path| std::fs::read(path).ok())
        else {
            continue;
        };
        let recorded = match record(&case, &provider) {
            Recording::Ready(recorded) => recorded,
            Recording::Done(status) => {
                report.push(&case.id, status);
                continue;
            }
        };
        let status = match Run::page(&recorded).reference(&recorded) {
            Err(error) => failed(None, Some(&error)),
            Ok(reference) => match parse_ppm(&oracle) {
                Some((width, height, expected))
                    if (width, height)
                        == (reference.region().width(), reference.region().height()) =>
                {
                    let actual = rgb_on_white(&reference.to_rgba8());
                    let worst = actual
                        .iter()
                        .zip(expected)
                        .enumerate()
                        .map(|(i, (&a, &e))| (a.abs_diff(e), i / 3))
                        .max();
                    match worst {
                        Some((difference, pixel)) if difference > case.tolerance => {
                            Status::Failed(format!(
                                "differs from the oracle by {difference} (tolerance {}) at pixel {pixel}",
                                case.tolerance
                            ))
                        }
                        _ => Status::Checked,
                    }
                }
                _ => Status::Failed("oracle size differs from the page".to_owned()),
            },
        };
        report.push(&case.id, status);
    }
    report.finish("oracles");
    let failures: Vec<_> = report.failures().iter().map(|e| e.id.clone()).collect();
    assert!(
        failures.is_empty(),
        "the reference misses fixture oracles: {failures:?}"
    );
}

#[test]
fn results_do_not_depend_on_band_height_partition_or_cache() {
    let provider = pagura_data::Bundled::default();
    let full = std::env::var_os("PAGURA_REFERENCE_FULL").is_some();
    let mut report = Report::default();
    for (index, case) in render_cases().into_iter().enumerate() {
        let recorded = match record(&case, &provider) {
            Recording::Ready(recorded) => recorded,
            Recording::Done(status) => {
                report.push(&case.id, status);
                continue;
            }
        };
        let probe_charges = full || index % 8 == 0;
        report.push(&case.id, invariance(&case, &recorded, probe_charges));
    }
    report.finish("invariance");
    let failures: Vec<_> = report.failures().iter().map(|e| e.id.clone()).collect();
    assert!(
        failures.is_empty(),
        "results depend on bands, partitions or the cache: {failures:?}"
    );
}

/// What a rendering gave, compared by value: pixels, or the error's kind,
/// object and position.
#[derive(Debug, Clone, PartialEq)]
enum Outcome {
    Pixels(Vec<u8>),
    Error(
        ErrorKind,
        Option<pagura_core::ObjectId>,
        pagura_core::Position,
    ),
}

fn outcome(result: pagura_core::Result<Vec<u8>>) -> Result<Outcome, Status> {
    match result {
        Ok(pixels) => Ok(Outcome::Pixels(pixels)),
        Err(error) if pending(&error) => Err(Status::Pending(describe(&error))),
        Err(error) => Ok(Outcome::Error(error.kind, error.object, error.position)),
    }
}

fn invariance(case: &Case, recorded: &Recorded, probe_charges: bool) -> Status {
    let page = Run::page(recorded);
    let base = match outcome(page.render(recorded)) {
        Ok(outcome) => outcome,
        Err(status) => return status,
    };
    let mut problems = Vec::new();
    for band_height in BAND_HEIGHTS {
        let run = Run {
            band_height,
            ..page
        };
        match outcome(run.render(recorded)) {
            Ok(outcome) if outcome == base => {}
            Ok(_) => problems.push(format!("band height {band_height} differs")),
            Err(status) => return status,
        }
    }
    let store = fresh();
    for pass in ["filling", "hitting"] {
        match outcome(page.render_with(recorded, &store)) {
            Ok(outcome) if outcome == base => {}
            Ok(_) => problems.push(format!("a store {pass} its values differs")),
            Err(status) => return status,
        }
    }
    for (number, tiles) in partitions(&case.id, recorded.page).into_iter().enumerate() {
        if let Some(problem) = partition(recorded, page, &base, &tiles) {
            problems.push(format!("partition {number}: {problem}"));
        }
    }
    if probe_charges {
        problems.extend(charges(recorded, page));
    }
    if problems.is_empty() {
        Status::Checked
    } else {
        Status::Failed(problems.join("; "))
    }
}

/// Checks that `tiles` assemble into `base`, or report its error.
fn partition(
    recorded: &Recorded,
    page: Run,
    base: &Outcome,
    tiles: &[DeviceRect],
) -> Option<String> {
    let width = usize::try_from(recorded.page.width()).ok()?;
    let mut assembled = vec![0_u8; width * usize::try_from(recorded.page.height()).ok()? * 4];
    let mut errors = Vec::new();
    for (i, &tile) in tiles.iter().enumerate() {
        let run = Run {
            region: tile,
            band_height: BAND_HEIGHTS[i % BAND_HEIGHTS.len()],
            ..page
        };
        match outcome(run.render(recorded)) {
            Ok(Outcome::Pixels(pixels)) => {
                let tile_width = usize::try_from(tile.width()).ok()? * 4;
                for (row, line) in pixels.chunks_exact(tile_width.max(1)).enumerate() {
                    let y = usize::try_from(tile.y0).ok()? + row;
                    let start = (y * width + usize::try_from(tile.x0).ok()?) * 4;
                    assembled
                        .get_mut(start..start + tile_width)?
                        .copy_from_slice(line);
                }
            }
            Ok(error) => errors.push(error),
            Err(_) => return None,
        }
    }
    match base {
        Outcome::Pixels(pixels) if !errors.is_empty() => {
            Some(format!("tiles fail where the page renders: {errors:?}"))
        }
        Outcome::Pixels(pixels) if *pixels != assembled => {
            Some("tiles do not assemble into the page".to_owned())
        }
        Outcome::Error(kind, ..) if *kind != ErrorKind::Limit && !errors.contains(base) => Some(
            format!("no tile reports the page's error {base:?}: {errors:?}"),
        ),
        _ => None,
    }
}

/// A budget's name and its field of [`Limits`].
type Budget = (&'static str, fn(&mut Limits) -> &mut usize);

/// Reserved charges must not depend on band height or on the store's
/// state: a hit costs what a miss costs.
fn charges(recorded: &Recorded, page: Run) -> Vec<String> {
    let budgets: [Budget; 3] = [
        ("render_work", |l| &mut l.render_work),
        ("render_bytes", |l| &mut l.render_bytes),
        ("function_work", |l| &mut l.function_work),
    ];
    let mut problems = Vec::new();
    for (name, field) in budgets {
        let base = page.charge(recorded, field, Cache::Fresh);
        let warm = page.charge(recorded, field, Cache::Warm);
        if warm != base {
            problems.push(format!(
                "{name} reserves {warm:?} with a filled store, {base:?} with an empty one"
            ));
        }
        for band_height in [1, 7, 16, 256] {
            let run = Run {
                band_height,
                ..page
            };
            let charge = run.charge(recorded, field, Cache::Fresh);
            if charge != base {
                problems.push(format!(
                    "{name} reserves {charge:?} at band height {band_height}, {base:?} at 32"
                ));
            }
        }
    }
    problems
}

/// Two random grid partitions of the page, seeded by the case id.
fn partitions(id: &str, page: DeviceRect) -> Vec<Vec<DeviceRect>> {
    let mut state = id.bytes().fold(0x9e37_79b9_7f4a_7c15_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut cuts = |low: u32, high: u32| {
        let mut cuts = vec![low, high];
        for _ in 0..2 {
            let span = u64::from(high - low);
            if span > 1 {
                cuts.push(low + u32::try_from(next() % span).unwrap_or(0));
            }
        }
        cuts.sort_unstable();
        cuts.dedup();
        cuts
    };
    (0..2)
        .map(|_| {
            let (xs, ys) = (cuts(page.x0, page.x1), cuts(page.y0, page.y1));
            let mut tiles = Vec::new();
            for y in ys.windows(2) {
                for x in xs.windows(2) {
                    tiles.push(DeviceRect {
                        x0: x[0],
                        y0: y[0],
                        x1: x[1],
                        y1: y[1],
                    });
                }
            }
            tiles
        })
        .collect()
}

#[test]
fn fast_parameters_alone_against_the_reference() {
    let provider = pagura_data::Bundled::default();
    let parameters = [
        (
            "glyph_origin_bins",
            FastParams {
                glyph_origin_bins: true,
                ..FastParams::EXACT
            },
        ),
        (
            "approximate_coverage",
            FastParams {
                approximate_coverage: true,
                ..FastParams::EXACT
            },
        ),
        (
            "byte_storage",
            FastParams {
                byte_storage: true,
                ..FastParams::EXACT
            },
        ),
        (
            "reduced_image_decode",
            FastParams {
                reduced_image_decode: true,
                ..FastParams::EXACT
            },
        ),
    ];
    let mut report = Report::default();
    for case in render_cases() {
        let recorded = match record(&case, &provider) {
            Recording::Ready(recorded) => recorded,
            Recording::Done(status) => {
                report.push(&case.id, status);
                continue;
            }
        };
        let page = Run::page(&recorded);
        let reference = page.reference(&recorded);
        for (name, fast) in parameters {
            let id = format!("{} {name}", case.id);
            let run = Run { fast, ..page };
            let status = match (run.render(&recorded), &reference) {
                (Ok(rendered), Ok(reference)) => {
                    Status::Compared(Comparison::of(&rendered, reference))
                }
                (rendered, reference) => failed(rendered.as_ref().err(), reference.as_ref().err()),
            };
            report.push(id, status);
        }
    }
    // Fast parameters are outside the one-level contract: reported only.
    report.finish("fast");
}
