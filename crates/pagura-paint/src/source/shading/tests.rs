//! Shading spans against the previous per-pixel sampling (`oracle`) for
//! every shading in the generated corpus, under several device transforms,
//! both sample rates, `sh` and pattern use, and two targets.

mod oracle;
mod parse;

use super::*;
use crate::source::{PaintContext, SpanColors};
use pagura_color::{Converter, Process, Target};
use pagura_core::{Limits, Name, Store};
use pagura_interpret::Composite;
use parse::{OldPrimitive, Parsed};
use std::sync::Arc;

/// The device grid sampled for each shading.
const SIZE: u32 = 40;

/// The pixels a shading is prepared for: as many as a page's, so that a
/// one-input stage certifies its colours ahead as it would there.
const PIXELS: usize = 1 << 17;

/// Prepares `parts` as preparation for [`PIXELS`] pixels does.
fn prepared(
    parts: &Parts<'_>,
    transform: Transform,
    context: &PaintContext<'_>,
    usage: Use,
) -> ShadingSource {
    let mut bytes = Meter::new(context.limits.render_bytes, "test bytes");
    let mut functions = Meter::new(usize::MAX, "test functions");
    ShadingSource::new(
        parts,
        transform,
        context,
        usage,
        (&mut bytes, &mut functions),
    )
    .unwrap()
    .reserve(PIXELS, (&mut bytes, &mut functions))
    .unwrap()
}

fn corpus() -> Vec<Parsed> {
    let root = pagura_testkit::workspace_root().join("corpus/generated");
    let mut stack = vec![root];
    let mut files = Vec::new();
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "pdf") {
                files.push(path);
            }
        }
    }
    files.sort();
    let mut shadings = Vec::new();
    for path in files {
        let bytes = std::fs::read(&path).unwrap();
        if !bytes.windows(12).any(|w| w == b"/ShadingType") {
            continue;
        }
        let Ok(document) =
            pagura_document::Document::open(bytes, &pagura_document::OpenOptions::default())
        else {
            continue;
        };
        for mut parsed in parse::shadings(&document) {
            parsed.name = format!(
                "{} {}",
                path.file_name().unwrap().to_string_lossy(),
                parsed.name
            );
            shadings.push(parsed);
        }
    }
    shadings
}

/// The region of shading space worth sampling, with a margin.
fn frame(parsed: &Parsed) -> [f64; 4] {
    let mut points: Vec<Point> = Vec::new();
    if let Some(b) = parsed.bbox {
        points.extend([Point::new(b.x0(), b.y0()), Point::new(b.x1(), b.y1())]);
    }
    match &parsed.kind {
        ShadingKind::Axial { start, end, .. } => points.extend([*start, *end]),
        ShadingKind::Radial {
            start,
            start_radius,
            end,
            end_radius,
            ..
        } => {
            for (c, r) in [(start, start_radius), (end, end_radius)] {
                points.extend([Point::new(c.x - r, c.y - r), Point::new(c.x + r, c.y + r)]);
            }
        }
        ShadingKind::Function { domain, matrix } => {
            for (x, y) in [
                (domain[0], domain[2]),
                (domain[1], domain[3]),
                (domain[0], domain[3]),
                (domain[1], domain[2]),
            ] {
                points.push(Point::new(x, y) * *matrix);
            }
        }
        _ => {
            for p in &parsed.old_mesh {
                match p {
                    OldPrimitive::Triangle(v) => points.extend(v.iter().map(|v| v.0)),
                    OldPrimitive::Patch { points: p, .. } => points.extend(p.iter().copied()),
                }
            }
        }
    }
    let lo = points
        .iter()
        .fold(Point::new(f64::INFINITY, f64::INFINITY), |a, p| {
            Point::new(a.x.min(p.x), a.y.min(p.y))
        });
    let hi = points
        .iter()
        .fold(Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY), |a, p| {
            Point::new(a.x.max(p.x), a.y.max(p.y))
        });
    let (w, h) = ((hi.x - lo.x).max(1.0), (hi.y - lo.y).max(1.0));
    [lo.x - 0.3 * w, lo.y - 0.3 * h, w * 1.6, h * 1.6]
}

/// Shading space to device: fitted with a PDF-style flip, unflipped, and
/// rotated.
fn transforms(parsed: &Parsed) -> Vec<Transform> {
    let [x0, y0, w, h] = frame(parsed);
    let s = f64::from(SIZE) / w.max(h);
    let flip = Transform::new(s, 0.0, 0.0, -s, -x0 * s, (y0 + h) * s);
    let plain = Transform::new(s, 0.0, 0.0, s, -x0 * s, -y0 * s);
    let (sin, cos) = 0.5_f64.sin_cos();
    let centre = Point::new(f64::from(SIZE) / 2.0, f64::from(SIZE) / 2.0);
    let rotate = Transform::translate(-centre.x, -centre.y)
        .then(Transform::new(cos, sin, -sin, cos, 0.0, 0.0))
        .then(Transform::translate(centre.x, centre.y));
    vec![flip, plain, flip.then(rotate)]
}

fn parts(parsed: &Parsed, antialias: bool) -> Parts<'_> {
    Parts {
        space: &parsed.space,
        function: parsed.function.as_ref(),
        kind: &parsed.kind,
        bbox: parsed.bbox,
        antialias,
        background: parsed.background.as_deref(),
    }
}

struct Counts {
    pixels: usize,
    failed: usize,
    aligned: usize,
    shadings: usize,
    /// Sources whose colours are certified ahead.
    certified: usize,
}

fn compare(parsed: &Parsed, target: &Target, counts: &mut Counts) {
    let store = Store::new(1 << 20);
    let limits = Limits::default();
    let composite = Composite::default();
    let context = PaintContext {
        transform: Transform::IDENTITY,
        target,
        composite: &composite,
        store: &store,
        limits: &limits,
        region: pagura_raster::DeviceRect {
            x0: 0,
            y0: 0,
            x1: u32::MAX,
            y1: u32::MAX,
        },
    };
    let Ok(converter) = Converter::new(&parsed.space, target, &composite.conversion) else {
        return;
    };
    counts.shadings += 1;
    for transform in transforms(parsed) {
        for antialias in [parsed.antialias, !parsed.antialias] {
            for usage in [Use::Operator, Use::Pattern] {
                let parts = parts(parsed, antialias);
                let source = prepared(&parts, transform, &context, usage);
                counts.aligned += usize::from(source.aligned.is_some());
                counts.certified += usize::from(source.colors.certified());
                let old = oracle::Oracle {
                    parsed,
                    converter: &converter,
                    inverse: transform.inverse().unwrap(),
                    rate: if antialias { 2 } else { 1 },
                    pattern: usage == Use::Pattern,
                    certified: source.pattern.as_ref().is_some_and(|p| p.bbox.is_none())
                        && parsed.bbox.is_some(),
                    index: oracle::Index::new(&parsed.old_mesh),
                };
                let boundary = source.bbox_use == BBoxUse::Boundary;
                let context = format!(
                    "{} {usage:?} aa {antialias} {transform:?} {target:?}",
                    parsed.name
                );
                compare_grid(&source, &old, boundary, &context, counts);
            }
        }
    }
}

fn compare_grid(
    source: &ShadingSource,
    old: &oracle::Oracle<'_>,
    boundary: bool,
    context: &str,
    counts: &mut Counts,
) {
    let mut out = SpanColors::default();
    let mut levels = SpanColors::default();
    let channels = source.channels;
    for y in 0..SIZE {
        // Two spans per row, so that spans start mid-row too.
        for (x0, x1) in [(0, SIZE / 3), (SIZE / 3, SIZE)] {
            let coverage: Vec<u8> = (x0..x1)
                .map(|x| ((x * 37 + y * 101) % 256).to_le_bytes()[0])
                .collect();
            for (out, precision) in [
                (&mut out, Precision::Exact),
                (&mut levels, Precision::Level),
            ] {
                if boundary {
                    source.span_with_boundary(x0, y, &coverage, precision, out);
                } else {
                    source.span(x0, y, (x1 - x0) as usize, precision, out);
                }
            }
            // Colours kept to F32 levels store the levels of the exact ones.
            let level = |v: &[f64]| {
                v.iter()
                    .map(|&v| crate::kernel::level(v).to_bits())
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                level(&levels.colors),
                level(&out.colors),
                "{context} row {y}: levels"
            );
            assert_eq!(levels.alpha, out.alpha, "{context} row {y}: level alpha");
            for x in x0..x1 {
                let i = (x - x0) as usize;
                let alpha = out.alpha.get(i).copied().unwrap_or(1.0);
                let color = &out.colors[i * channels..(i + 1) * channels];
                let cover = boundary.then(|| f64::from(coverage[i]) / 255.0);
                match old.pixel((x, y), cover) {
                    Err(_) => counts.failed += 1,
                    Ok(None) => {
                        counts.pixels += 1;
                        assert_eq!(alpha, 0.0, "{context} at {x},{y}: covered, previously not");
                    }
                    Ok(Some((expected, shape))) => {
                        counts.pixels += 1;
                        assert_eq!(
                            alpha.to_bits(),
                            shape.to_bits(),
                            "{context} at {x},{y}: alpha"
                        );
                        let bits = |v: &[f64]| v.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
                        assert_eq!(
                            bits(color),
                            bits(&expected),
                            "{context} at {x},{y}: {color:?} != {expected:?}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn corpus_shadings_match_the_previous_sampling() {
    let shadings = corpus();
    let mut counts = Counts {
        pixels: 0,
        failed: 0,
        aligned: 0,
        shadings: 0,
        certified: 0,
    };
    let rgb = Target::new(Process::Rgb);
    let cmyk = Target::separated(
        Process::Cmyk,
        Arc::from([Name::new(b"Spot"), Name::new(b"PANTONE 300")]),
    );
    for parsed in &shadings {
        compare(parsed, &rgb, &mut counts);
        compare(parsed, &cmyk, &mut counts);
    }
    assert!(shadings.len() >= 60, "{} shadings", shadings.len());
    assert!(
        counts.pixels > 1_000_000 && counts.aligned > 50 && counts.certified > 50,
        "{} pixels, {} failed, {} aligned, {} certified, {} shadings",
        counts.pixels,
        counts.failed,
        counts.aligned,
        counts.certified,
        counts.shadings
    );
}

mod synthetic;
