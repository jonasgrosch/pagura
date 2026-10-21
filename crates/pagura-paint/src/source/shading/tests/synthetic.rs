//! Random analytic shadings under transforms aligned with the device grid
//! (including axes through sample points, where the shared evaluation must
//! fall back) and rotated ones, against the previous sampling; constant
//! collapse; solid paint; and the function-work reservation.
#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "small random values"
)]

use super::oracle::{Index, Oracle};
use super::parse::Parsed;
use super::*;
use crate::source::{PaintContext, Source};
use pagura_color::{ColorSpace, Components, ExponentialFunction, Function, Process, SampledFunction, Target};
use pagura_core::{ErrorKind, Limits, Meter, Name, Store};
use pagura_interpret::{Color, Composite, Overprint, Paint, ShadingFunction};
use std::sync::Arc;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    /// A multiple of 1/4 in `low..high`.
    fn quarter(&mut self, low: f64, high: f64) -> f64 {
        low + (self.below(((high - low) * 4.0) as u64 + 1) as f64) / 4.0
    }
}

fn function(rng: &mut Rng, outputs: usize) -> Arc<Function> {
    if rng.below(3) == 0 {
        let size = 2 + rng.below(6) as u32;
        let samples: Vec<u8> = (0..size as usize * outputs).map(|_| rng.next() as u8).collect();
        return Arc::new(
            Function::sampled(
                SampledFunction {
                    domain: vec![[0.0, 1.0]],
                    range: vec![[0.0, 1.0]; outputs],
                    size: vec![size],
                    bits_per_sample: 8,
                    order: 1,
                    encode: None,
                    decode: None,
                    samples: samples.into(),
                },
                &Limits::default(),
            )
            .unwrap(),
        );
    }
    let c0: Vec<f64> = (0..outputs).map(|_| rng.quarter(0.0, 1.0)).collect();
    let c1 = if rng.below(4) == 0 {
        c0.clone()
    } else {
        (0..outputs).map(|_| rng.below(1000) as f64 / 999.0).collect()
    };
    Arc::new(
        Function::exponential(ExponentialFunction {
            domain: [0.0, 1.0],
            range: None,
            c0,
            c1,
            exponent: [1.0, 2.0, 0.5][rng.below(3) as usize],
        })
        .unwrap(),
    )
}

fn shading(rng: &mut Rng) -> Parsed {
    let (space, n) = [
        (ColorSpace::DeviceRgb, 3),
        (ColorSpace::DeviceGray, 1),
        (ColorSpace::DeviceCmyk, 4),
    ][rng.below(3) as usize]
        .clone();
    let extend = [rng.below(2) == 0, rng.below(2) == 0];
    let domain = if rng.below(4) == 0 { [0.25, 0.75] } else { [0.0, 1.0] };
    let point = |rng: &mut Rng| Point::new(rng.quarter(0.0, 16.0), rng.quarter(0.0, 16.0));
    let kind = match rng.below(8) {
        0..=3 => {
            let start = point(rng);
            // Mostly axes along a coordinate axis.
            let end = match rng.below(3) {
                0 => Point::new(rng.quarter(0.0, 16.0), start.y),
                1 => Point::new(start.x, rng.quarter(0.0, 16.0)),
                _ => point(rng),
            };
            ShadingKind::Axial { start, end, domain, extend }
        }
        4..=6 => ShadingKind::Radial {
            start: point(rng),
            start_radius: rng.quarter(0.0, 4.0),
            end: point(rng),
            end_radius: rng.quarter(0.0, 8.0),
            domain,
            extend,
        },
        _ => ShadingKind::Function {
            domain: [0.0, 1.0, 0.0, 1.0],
            matrix: Transform::new(16.0, 0.0, 0.0, 16.0, 0.0, 0.0),
        },
    };
    let (space, function) = if matches!(kind, ShadingKind::Function { .. }) {
        let program = pagura_color::CalculatorFunction {
            domain: vec![[0.0, 1.0]; 2],
            range: vec![[0.0, 1.0]; 3],
            program: b"{ add 2 div dup 1 exch sub 0.5 }".to_vec().into(),
        };
        let function = Function::calculator(program, &Limits::default()).unwrap();
        (ColorSpace::DeviceRgb, Arc::new(function))
    } else {
        (space, function(rng, n))
    };
    Parsed {
        name: String::from("synthetic"),
        space: Arc::new(space),
        function: Some(ShadingFunction::Single(function)),
        kind,
        bbox: None,
        antialias: false,
        background: None,
        old_mesh: Vec::new(),
    }
}

fn transform(rng: &mut Rng) -> Transform {
    let s = [1.0, 2.0, 0.5, 4.0, 1.5][rng.below(5) as usize];
    let (tx, ty) = (rng.quarter(-4.0, 4.0), rng.quarter(-4.0, 36.0));
    match rng.below(4) {
        0 => Transform::new(s, 0.0, 0.0, s, tx, ty),
        1 => Transform::new(s, 0.0, 0.0, -s, tx, ty + 32.0),
        2 => Transform::new(0.0, s, s, 0.0, tx, ty),
        _ => Transform::new(s * 0.8, s * 0.6, -s * 0.6, s * 0.8, tx + 16.0, ty),
    }
}

fn context<'a>(target: &'a Target, composite: &'a Composite, store: &'a Store, limits: &'a Limits) -> PaintContext<'a> {
    PaintContext {
        transform: Transform::IDENTITY,
        target,
        composite,
        store,
        limits,
        reduced_image_decode: false,
    }
}

#[test]
fn random_shadings_match_the_previous_sampling() {
    let mut rng = Rng(0x5ade);
    let target = Target::new(Process::Rgb);
    let (composite, store, limits) = (Composite::default(), Store::new(1 << 20), Limits::default());
    let context = context(&target, &composite, &store, &limits);
    let mut aligned = 0;
    let mut counts = Counts { pixels: 0, failed: 0, aligned: 0, shadings: 0 };
    for case in 0..400 {
        let parsed = shading(&mut rng);
        let transform = transform(&mut rng);
        let converter = Converter::new(&parsed.space, &target, &composite.conversion).unwrap();
        for antialias in [false, true] {
            for usage in [Use::Operator, Use::Pattern] {
                let source =
                    ShadingSource::new(&parts(&parsed, antialias), transform, &context, usage).unwrap();
                aligned += usize::from(source.aligned.is_some());
                let old = Oracle {
                    parsed: &parsed,
                    converter: &converter,
                    inverse: transform.inverse().unwrap(),
                    rate: if antialias { 2 } else { 1 },
                    pattern: usage == Use::Pattern,
                    certified: false,
                    index: Index::new(&[]),
                };
                let context = format!("case {case}: {:?} under {transform:?} aa {antialias}", parsed.kind);
                compare_grid(&source, &old, false, &context, &mut counts);
            }
        }
    }
    assert!(aligned > 150 && counts.pixels > 1_000_000, "{aligned} {}", counts.pixels);
}

#[test]
fn constant_shadings_collapse_to_the_previous_colour() {
    let target = Target::new(Process::Rgb);
    let (composite, store, limits) = (Composite::default(), Store::new(1 << 20), Limits::default());
    let context = context(&target, &composite, &store, &limits);
    let function = Arc::new(
        Function::exponential(ExponentialFunction {
            domain: [0.0, 1.0],
            range: None,
            c0: vec![0.1, 0.2, 0.3],
            c1: vec![0.1, 0.2, 0.3],
            exponent: 1.0,
        })
        .unwrap(),
    );
    for antialias in [false, true] {
        let parsed = Parsed {
            name: String::new(),
            space: Arc::new(ColorSpace::DeviceRgb),
            function: Some(ShadingFunction::Single(function.clone())),
            kind: ShadingKind::Axial {
                start: Point::new(0.0, 0.0),
                end: Point::new(512.0, 0.0),
                domain: [0.0, 1.0],
                extend: [true, true],
            },
            bbox: None,
            antialias,
            background: None,
            old_mesh: Vec::new(),
        };
        let transform = Transform::new(1.0, 0.0, 0.0, -1.0, 0.0, 512.0);
        let source = ShadingSource::new(&parts(&parsed, antialias), transform, &context, Use::Operator).unwrap();
        let constant = source.constant().unwrap().to_vec();
        let converter = Converter::new(&parsed.space, &target, &composite.conversion).unwrap();
        let old = Oracle {
            parsed: &parsed,
            converter: &converter,
            inverse: transform.inverse().unwrap(),
            rate: if antialias { 2 } else { 1 },
            pattern: false,
            certified: false,
            index: Index::new(&[]),
        };
        for (x, y) in [(0, 0), (100, 7), (511, 511), (4000, 3)] {
            let (expected, alpha) = old.pixel((x, y), None).unwrap().unwrap();
            assert_eq!(alpha, 1.0);
            assert_eq!(constant.iter().map(|v| v.to_bits()).collect::<Vec<_>>(), expected.iter().map(|v| v.to_bits()).collect::<Vec<_>>());
        }
    }
}

#[test]
fn solid_paint_is_converted_once_with_its_overprint_channels() {
    let target = Target::separated(Process::Cmyk, Arc::from([Name::new(b"Spot")].as_slice()));
    let (store, limits) = (Store::new(1 << 20), Limits::default());
    let paint = Paint::Solid(Color {
        space: Arc::new(ColorSpace::DeviceCmyk),
        components: Components::from_slice(&[0.5, 0.0, 0.25, 0.0]),
    });
    let mut composite = Composite::default();
    let expected = [0.5, 1.0, 0.75, 1.0, 1.0];
    for (overprint, painted) in [
        (Overprint { enabled: false, nonzero: false }, 0b11111),
        (Overprint { enabled: true, nonzero: false }, 0b01111),
        (Overprint { enabled: true, nonzero: true }, 0b00101),
    ] {
        composite.overprint = overprint;
        let context = context(&target, &composite, &store, &limits);
        let mut functions = Meter::new(10, "function work");
        let source = Source::prepare(&paint, &context, 100, &mut functions).unwrap();
        let Source::Solid { color, painted: actual } = &source else {
            panic!("not solid");
        };
        assert_eq!(&**color, &expected);
        assert_eq!(actual.0, painted, "{overprint:?}");
        assert_eq!(functions.used(), 0);
    }
}

#[test]
fn function_work_is_reserved_per_sample() {
    let target = Target::new(Process::Rgb);
    let (composite, store, limits) = (Composite::default(), Store::new(1 << 20), Limits::default());
    let context = context(&target, &composite, &store, &limits);
    let mut rng = Rng(3);
    let mut parsed = shading(&mut rng);
    while !matches!(parsed.kind, ShadingKind::Radial { .. }) {
        parsed = shading(&mut rng);
    }
    let shading = parts(&parsed, true);
    let cost = match parsed.function.as_ref().unwrap() {
        ShadingFunction::Single(f) => f.cost(),
        ShadingFunction::PerComponent(_) => unreachable!(),
    };
    let prepare = || ShadingSource::new(&shading, Transform::IDENTITY, &context, Use::Operator).unwrap();
    let mut meter = Meter::new(usize::MAX, "function work");
    prepare().reserve(1000, &mut meter).unwrap();
    assert_eq!(meter.used(), 1000 * 4 * cost);
    let mut tight = Meter::new(1000 * 4 * cost - 1, "function work");
    let error = prepare().reserve(1000, &mut tight).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
}

/// Times the span fill of a 512² page of an axial shading along the page
/// width (the stress set's `const`/`vary` inputs at 72 dpi), aligned and
/// rotated, with the shared evaluation and without it. Run in release
/// under the bench lock:
/// `cargo test --release -p pagura-paint axial_span_timing -- --ignored --nocapture`.
#[test]
#[ignore = "timing, run explicitly in release"]
fn axial_span_timing() {
    let target = Target::new(Process::Rgb);
    let (composite, store, limits) = (Composite::default(), Store::new(1 << 20), Limits::default());
    let context = context(&target, &composite, &store, &limits);
    let flip = Transform::new(1.0, 0.0, 0.0, -1.0, 0.0, 512.0);
    let (sin, cos) = 0.3_f64.sin_cos();
    let rotated = Transform::new(cos, sin, -sin, cos, 40.0, 0.0).then(flip);
    for (name, c1) in [("const", [0.0, 0.0, 0.0]), ("vary", [1.0, 0.0, 0.0])] {
        let function = Arc::new(
            Function::exponential(ExponentialFunction {
                domain: [0.0, 1.0],
                range: None,
                c0: vec![0.0; 3],
                c1: c1.to_vec(),
                exponent: 1.0,
            })
            .unwrap(),
        );
        let parsed = Parsed {
            name: String::new(),
            space: Arc::new(ColorSpace::DeviceRgb),
            function: Some(ShadingFunction::Single(function)),
            kind: ShadingKind::Axial {
                start: Point::new(0.0, 0.0),
                end: Point::new(512.0, 0.0),
                domain: [0.0, 1.0],
                extend: [true, true],
            },
            bbox: None,
            antialias: false,
            background: None,
            old_mesh: Vec::new(),
        };
        for (variant, transform, shared) in [
            ("aligned", flip, true),
            ("aligned, no sharing", flip, false),
            ("rotated", rotated, true),
        ] {
            let mut times = Vec::new();
            let mut out = SpanColors::default();
            let mut checksum = 0.0;
            for _ in 0..15 {
                let start = std::time::Instant::now();
                let mut source =
                    ShadingSource::new(&parts(&parsed, false), transform, &context, Use::Operator).unwrap();
                if !shared {
                    source.aligned = None;
                    source.constant = None;
                }
                let constant = source.constant().is_some();
                for y in 0..512 {
                    if constant {
                        // A collapsed shading is a solid: the executor fills
                        // it without calling the source per span.
                        break;
                    }
                    source.span(0, y, 512, &mut out);
                    checksum += out.colors[y as usize % 512];
                }
                times.push(start.elapsed().as_secs_f64() * 1e3);
            }
            times.sort_by(f64::total_cmp);
            eprintln!(
                "{name} {variant}: median {:.3} ms, min {:.3} ms per 512² page ({checksum:.1})",
                times[times.len() / 2],
                times[0]
            );
        }
    }
}
