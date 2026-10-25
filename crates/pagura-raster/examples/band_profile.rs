#![forbid(unsafe_code)]
//! Times coverage generation of the band rasterizer on many small strokes
//! and fills and on large fills, or on the marks of a page read from a file.
//! Coverage only: nothing is composited. Timings are hypotheses until
//! measured under the performance policy (`docs/ARCHITECTURE.md`).
//!
//! `cargo run --release -p pagura-raster --example band_profile`
#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "page sizes and style indices are small nonnegative numbers"
)]

use pagura_core::{Limits, Meter};
use pagura_raster::{
    Adjustment, Area, BandCoverage, BandRasterizer, DeviceRect, FillRule, LineCap, LineJoin,
    MarkCoverage, Path, PathBuilder, Point, Shape, ShapeOptions, Span, Stroke, Transform,
};
use std::error::Error;
use std::time::{Duration, Instant};

struct Lcg(u64);

impl Lcg {
    fn unit(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        f64::from(u32::try_from(self.0 >> 33).unwrap_or(0)) / f64::from(1_u32 << 31)
    }
}

enum Mark {
    Fill(Path, FillRule),
    Stroke(Path, Stroke, Transform),
}

/// Marks listed one per line: `F N|E segments` or
/// `S width cap join miter dashes|- phase a b c d e f segments`, with
/// segments `M x y`, `L x y`, `C x1 y1 x2 y2 x y` and `Z` in default user
/// space, and the stroke's CTM as its pen.
fn read(file: &str) -> Result<Vec<Mark>, Box<dyn Error>> {
    let mut marks = Vec::new();
    for line in std::fs::read_to_string(file)?.lines() {
        let mut words = line.split_whitespace();
        let kind = words.next().unwrap_or_default();
        let number = |words: &mut std::str::SplitWhitespace<'_>| -> Result<f64, Box<dyn Error>> {
            Ok(words.next().ok_or("short line")?.parse::<f64>()?)
        };
        let (rule, stroke, pen) = if kind == "F" {
            let rule = if words.next() == Some("E") {
                FillRule::EvenOdd
            } else {
                FillRule::NonZero
            };
            (rule, None, Transform::IDENTITY)
        } else {
            let width = number(&mut words)?;
            let cap = [LineCap::Butt, LineCap::Round, LineCap::ProjectingSquare]
                [number(&mut words)? as usize % 3];
            let join = [LineJoin::Miter, LineJoin::Round, LineJoin::Bevel]
                [number(&mut words)? as usize % 3];
            let miter_limit = number(&mut words)?;
            let dashes = words.next().ok_or("short line")?;
            let dash_array = if dashes == "-" {
                Vec::new()
            } else {
                dashes
                    .split(',')
                    .map(str::parse::<f64>)
                    .collect::<Result<_, _>>()?
            };
            let dash_phase = number(&mut words)?;
            let m: Vec<f64> = (0..6)
                .map(|_| number(&mut words))
                .collect::<Result<_, _>>()?;
            let stroke = Stroke {
                width,
                cap,
                join,
                miter_limit,
                dash_array,
                dash_phase,
            };
            (
                FillRule::NonZero,
                Some(stroke),
                Transform::new(m[0], m[1], m[2], m[3], m[4], m[5]),
            )
        };
        let mut path = PathBuilder::new();
        while let Some(op) = words.next() {
            let point =
                |words: &mut std::str::SplitWhitespace<'_>| -> Result<Point, Box<dyn Error>> {
                    let x = number(words)?;
                    Ok(Point::new(x, number(words)?))
                };
            match op {
                "M" => {
                    path.move_to(point(&mut words)?);
                }
                "L" => {
                    path.line_to(point(&mut words)?)?;
                }
                "C" => {
                    let (a, b, c) = (point(&mut words)?, point(&mut words)?, point(&mut words)?);
                    path.cubic_to(a, b, c)?;
                }
                _ => {
                    path.close();
                }
            }
        }
        marks.push(match stroke {
            Some(stroke) => Mark::Stroke(path.finish(), stroke, pen),
            None => Mark::Fill(path.finish(), rule),
        });
    }
    Ok(marks)
}

fn strokes(count: usize) -> Result<Vec<Mark>, Box<dyn Error>> {
    let mut rng = Lcg(1);
    let mut marks = Vec::new();
    for _ in 0..count {
        let x = rng.unit() * 600.0;
        let y = rng.unit() * 780.0;
        let mut path = PathBuilder::new();
        path.move_to(Point::new(x, y));
        path.line_to(Point::new(
            x + rng.unit() * 20.0 - 10.0,
            y + rng.unit() * 20.0 - 10.0,
        ))?;
        path.line_to(Point::new(
            x + rng.unit() * 20.0 - 10.0,
            y + rng.unit() * 20.0 - 10.0,
        ))?;
        let stroke = Stroke {
            width: 0.3 + rng.unit() * 1.5,
            cap: LineCap::Butt,
            join: LineJoin::Miter,
            ..Stroke::default()
        };
        marks.push(Mark::Stroke(path.finish(), stroke, Transform::IDENTITY));
    }
    Ok(marks)
}

fn small_fills(count: usize) -> Result<Vec<Mark>, Box<dyn Error>> {
    let mut rng = Lcg(2);
    let mut marks = Vec::new();
    for i in 0..count {
        let x = rng.unit() * 600.0;
        let y = rng.unit() * 780.0;
        let mut path = PathBuilder::new();
        path.move_to(Point::new(x, y));
        for _ in 0..2 + i % 3 {
            path.line_to(Point::new(x + rng.unit() * 12.0, y + rng.unit() * 12.0))?;
        }
        path.close();
        marks.push(Mark::Fill(path.finish(), FillRule::NonZero));
    }
    Ok(marks)
}

fn large_fills(count: usize) -> Result<Vec<Mark>, Box<dyn Error>> {
    let mut rng = Lcg(3);
    let mut marks = Vec::new();
    for _ in 0..count {
        let (cx, cy) = (306.0 + rng.unit() * 40.0, 396.0 + rng.unit() * 40.0);
        let mut path = PathBuilder::new();
        let point = |angle: f64, radius: f64| {
            Point::new(cx + radius * angle.cos(), cy + radius * angle.sin())
        };
        path.move_to(point(0.0, 300.0));
        let n = 200;
        for k in 1..=n {
            let a = std::f64::consts::TAU * f64::from(k) / f64::from(n);
            let r = 250.0 + rng.unit() * 100.0;
            let c = point(a - 0.02, r + 30.0);
            path.cubic_to(c, c, point(a, if k == n { 300.0 } else { r }))?;
        }
        path.close();
        marks.push(Mark::Fill(path.finish(), FillRule::EvenOdd));
    }
    Ok(marks)
}

/// Rasterizes every mark at `scale` over `area`; the sum of the coverage.
fn rasterize(marks: &[Mark], scale: f64, area: &Area) -> Result<u64, Box<dyn Error>> {
    let limits = Limits::default();
    let mut bytes = Meter::new(usize::MAX, "bytes");
    let mut raster = BandRasterizer::new(*area, &mut bytes)?;
    let mut band = BandCoverage::with_capacity(pagura_raster::Capacity::band(area), &mut bytes)?;
    let options = ShapeOptions {
        area: *area,
        limits: &limits,
        scratch: None,
    };
    let device = Transform::new(scale, 0.0, 0.0, -scale, 0.0, f64::from(area.region.y1));
    let mut sum = 0_u64;
    for mark in marks {
        let shape = match mark {
            Mark::Fill(path, rule) => Shape::fill(path, device, *rule, &options)?,
            Mark::Stroke(path, stroke, pen) => Shape::stroke(
                path,
                stroke,
                pen.inverse().ok_or("singular pen")?,
                device,
                Adjustment::None,
                &options,
            )?,
        };
        let Some(shape) = shape else { continue };
        for b in shape.bands(area) {
            raster.rasterize(
                MarkCoverage {
                    shape: Some(&shape),
                    clip: None,
                },
                b,
                &mut band,
            );
            for row in band.rows() {
                for span in row.spans() {
                    sum += match span {
                        Span::Full { len, .. } => u64::from(len) * 255,
                        Span::Partial { coverage, .. } => {
                            coverage.iter().map(|&v| u64::from(v)).sum()
                        }
                    };
                }
            }
        }
    }
    Ok(sum)
}

/// The best of seven runs and the coverage sum.
fn best(marks: &[Mark], scale: f64, area: &Area) -> Result<(Duration, u64), Box<dyn Error>> {
    let mut best = Duration::MAX;
    let mut sum = 0;
    for _ in 0..7 {
        let start = Instant::now();
        sum = rasterize(marks, scale, area)?;
        best = best.min(start.elapsed());
    }
    Ok((best, sum))
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut workloads = vec![
        (
            "10,000 two-segment strokes".to_string(),
            strokes(10_000)?,
            (612.0, 792.0),
        ),
        (
            "10,000 small fills".to_string(),
            small_fills(10_000)?,
            (612.0, 792.0),
        ),
        (
            "20 large curved fills".to_string(),
            large_fills(20)?,
            (612.0, 792.0),
        ),
    ];
    // A page's marks as a file (see `read`) and its size in points.
    let mut args = std::env::args().skip(1);
    if let (Some(file), Some(width), Some(height)) = (args.next(), args.next(), args.next()) {
        workloads = vec![(
            file.clone(),
            read(&file)?,
            (width.parse()?, height.parse()?),
        )];
    }
    if let Ok(index) = std::env::var("LOOP") {
        let index = index.parse::<usize>().unwrap_or(0).min(workloads.len() - 1);
        let (marks, (w, h)) = (&workloads[index].1, workloads[index].2);
        let area = Area {
            region: DeviceRect {
                x0: 0,
                y0: 0,
                x1: w as u32,
                y1: h as u32,
            },
            band_height: 16,
        };
        let mut sum = 0;
        for _ in 0..100 {
            sum += rasterize(marks, 1.0, &area)?;
        }
        println!("{sum}");
        return Ok(());
    }
    for (name, marks, (width, height)) in &workloads {
        for (dpi, band_height) in [(72.0, 16), (150.0, 16), (300.0, 16), (300.0, 32)] {
            let scale = dpi / 72.0;
            let area = Area {
                region: DeviceRect {
                    x0: 0,
                    y0: 0,
                    x1: (width * scale) as u32,
                    y1: (height * scale) as u32,
                },
                band_height,
            };
            let (time, sum) = best(marks, scale, &area)?;
            println!(
                "{name} at {dpi} dpi, bands of {band_height}: {:.2} ms; coverage sum {sum}",
                time.as_secs_f64() * 1e3
            );
        }
    }
    Ok(())
}
