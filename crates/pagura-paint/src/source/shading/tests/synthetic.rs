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
use pagura_color::{
    ColorSpace, Components, ExponentialFunction, Function, Process, SampledFunction,
    StitchingFunction, Target,
};
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
        let samples: Vec<u8> = (0..size as usize * outputs)
            .map(|_| rng.next() as u8)
            .collect();
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
        (0..outputs)
            .map(|_| rng.below(1000) as f64 / 999.0)
            .collect()
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
    let domain = if rng.below(4) == 0 {
        [0.25, 0.75]
    } else {
        [0.0, 1.0]
    };
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
            ShadingKind::Axial {
                start,
                end,
                domain,
                extend,
            }
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

fn context<'a>(
    target: &'a Target,
    composite: &'a Composite,
    store: &'a Store,
    limits: &'a Limits,
) -> PaintContext<'a> {
    PaintContext {
        transform: Transform::IDENTITY,
        target,
        composite,
        store,
        limits,
        region: DeviceRect {
            x0: 0,
            y0: 0,
            x1: u32::MAX,
            y1: u32::MAX,
        },
    }
}

#[test]
fn random_shadings_match_the_previous_sampling() {
    let mut rng = Rng(0x5ade);
    let target = Target::new(Process::Rgb);
    let (composite, store, limits) = (Composite::default(), Store::new(1 << 20), Limits::default());
    let context = context(&target, &composite, &store, &limits);
    let mut aligned = 0;
    let mut counts = Counts {
        pixels: 0,
        failed: 0,
        aligned: 0,
        shadings: 0,
        certified: 0,
    };
    for case in 0..400 {
        let parsed = shading(&mut rng);
        let transform = transform(&mut rng);
        let converter = Converter::new(&parsed.space, &target, &composite.conversion).unwrap();
        for antialias in [false, true] {
            for usage in [Use::Operator, Use::Pattern] {
                let source = prepared(&parts(&parsed, antialias), transform, &context, usage);
                aligned += usize::from(source.aligned.is_some());
                counts.certified += usize::from(source.colors.certified());
                let old = Oracle {
                    parsed: &parsed,
                    converter: &converter,
                    inverse: transform.inverse().unwrap(),
                    rate: if antialias { 2 } else { 1 },
                    pattern: usage == Use::Pattern,
                    certified: false,
                    index: Index::new(&[]),
                };
                let context = format!(
                    "case {case}: {:?} under {transform:?} aa {antialias}",
                    parsed.kind
                );
                compare_grid(&source, &old, false, &context, &mut counts);
            }
        }
    }
    assert!(
        aligned > 150 && counts.pixels > 1_000_000 && counts.certified > 100,
        "{aligned} {} {}",
        counts.pixels,
        counts.certified
    );
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
        let source = ShadingSource::new(
            &parts(&parsed, antialias),
            transform,
            &context,
            Use::Operator,
            (
                &mut Meter::new(limits.render_bytes, "test bytes"),
                &mut Meter::new(usize::MAX, "test functions"),
            ),
        )
        .unwrap();
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
            assert_eq!(
                constant.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                expected.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
            );
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
        (
            Overprint {
                enabled: false,
                nonzero: false,
            },
            0b11111,
        ),
        (
            Overprint {
                enabled: true,
                nonzero: false,
            },
            0b01111,
        ),
        (
            Overprint {
                enabled: true,
                nonzero: true,
            },
            0b00101,
        ),
    ] {
        composite.overprint = overprint;
        let context = context(&target, &composite, &store, &limits);
        let mut functions = Meter::new(10, "function work");
        let source = Source::prepare(
            &paint,
            &context,
            100,
            (
                &mut functions,
                &Meter::new(limits.render_bytes, "test bytes"),
            ),
        )
        .unwrap();
        let Source::Solid {
            color,
            painted: actual,
        } = &source
        else {
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
    let prepare = || {
        ShadingSource::new(
            &shading,
            Transform::IDENTITY,
            &context,
            Use::Operator,
            (
                &mut Meter::new(limits.render_bytes, "test bytes"),
                &mut Meter::new(usize::MAX, "test functions"),
            ),
        )
        .unwrap()
    };
    let mut bytes = Meter::new(limits.render_bytes, "test bytes");
    let mut meter = Meter::new(usize::MAX, "function work");
    prepare()
        .reserve((1000, context.region), (&mut bytes, &mut meter))
        .unwrap();
    assert_eq!(meter.used(), 1000 * 4 * cost);
    let mut tight = Meter::new(1000 * 4 * cost - 1, "function work");
    let error = prepare()
        .reserve((1000, context.region), (&mut bytes, &mut tight))
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
}

#[test]
fn refused_ramp_storage_keeps_attempted_function_work() {
    let target = Target::new(Process::Rgb);
    let (composite, store, limits) = (Composite::default(), Store::new(1 << 20), Limits::default());
    let context = context(&target, &composite, &store, &limits);
    let function = Function::sampled(
        SampledFunction {
            domain: vec![[0.0, 1.0]],
            range: vec![[0.0, 1.0]; 3],
            size: vec![64],
            bits_per_sample: 8,
            order: 1,
            encode: Some(vec![[63.0, 0.0]]),
            decode: None,
            samples: (0..192).map(|v| v as u8).collect::<Vec<_>>().into(),
        },
        &limits,
    )
    .unwrap();
    let mut parsed = shading(&mut Rng(7));
    parsed.space = Arc::new(ColorSpace::DeviceRgb);
    parsed.function = Some(ShadingFunction::Single(Arc::new(function)));
    let build = |max| {
        let mut colors = Colors::new(
            &parts(&parsed, false),
            &context,
            &mut Meter::new(usize::MAX, "color bytes"),
        )
        .unwrap();
        let mut bytes = Meter::new(max, "ramp bytes");
        let mut functions = Meter::new(usize::MAX, "ramp work");
        colors.certify([0.0, 1.0], 64 * 32, (&mut bytes, &mut functions));
        (colors.certified(), bytes.used(), functions.used())
    };
    let (mut low, mut high) = (0, 1 << 20);
    assert!(build(high).0);
    while low + 1 < high {
        let mid = low + (high - low) / 2;
        if build(mid).0 {
            high = mid;
        } else {
            low = mid;
        }
    }
    let (certified, bytes, work) = build(low);
    assert!(!certified && bytes == 0);
    assert!(
        work > 0,
        "failed construction must still charge function work"
    );
}

#[test]
fn aligned_reservations_cover_every_evaluated_pixel() {
    let mut rng = Rng(0xa119);
    let target = Target::new(Process::Rgb);
    let (composite, store, limits) = (Composite::default(), Store::new(1 << 20), Limits::default());
    let context = context(&target, &composite, &store, &limits);
    let (mut columns, mut smaller) = (0, 0);
    for _ in 0..400 {
        let parsed = shading(&mut rng);
        let transform = transform(&mut rng);
        for antialias in [false, true] {
            let source = ShadingSource::new(
                &parts(&parsed, antialias),
                transform,
                &context,
                Use::Operator,
                (
                    &mut Meter::new(limits.render_bytes, "test bytes"),
                    &mut Meter::new(usize::MAX, "test functions"),
                ),
            )
            .unwrap();
            let (Geometry::Axial(axial), Some(aligned)) = (&*source.geometry, &source.aligned)
            else {
                continue;
            };
            // A sparse mark on a huge region must not scan its unused
            // columns. The full pixel reservation remains conservative.
            let huge = DeviceRect {
                x0: 0,
                y0: 0,
                x1: u32::MAX,
                y1: 1,
            };
            for pixels in [0, 1, 31, 1024] {
                assert_eq!(aligned.evaluated(&source, axial, huge, pixels), pixels);
                let empty = DeviceRect { y1: 0, ..huge };
                assert_eq!(aligned.evaluated(&source, axial, empty, pixels), 0);
            }
            for _ in 0..8 {
                let (x0, y0) = (rng.below(48) as u32, rng.below(48) as u32);
                let region = DeviceRect {
                    x0,
                    y0,
                    x1: x0 + 1 + rng.below(40) as u32,
                    y1: y0 + 1 + rng.below(40) as u32,
                };
                let area = (region.width() * region.height()) as usize;
                let reserved = aligned.evaluated(&source, axial, region, area);
                let evaluated = (region.y0..region.y1)
                    .flat_map(|y| (region.x0..region.x1).map(move |x| (x, y)))
                    .filter(|&pixel| aligned.evaluates(&source, axial, pixel))
                    .count();
                assert!(
                    evaluated <= reserved,
                    "{evaluated} > {reserved} in {region:?}"
                );
                columns += usize::from(aligned.pixels() > 2);
                smaller += usize::from(reserved < area);
            }
        }
    }
    assert!(
        columns > 100 && smaller > 100,
        "{columns} column regions, {smaller} smaller"
    );
}

#[test]
fn aligned_tables_are_reserved_before_building_and_match_retained_bytes() {
    let target = Target::new(Process::Rgb);
    let (composite, store, limits) = (Composite::default(), Store::new(1 << 20), Limits::default());
    let context = context(&target, &composite, &store, &limits);
    let mut parsed = shading(&mut Rng(7));
    parsed.kind = ShadingKind::Axial {
        start: Point::new(0.0, 0.0),
        end: Point::new(65_000.0, 0.0),
        domain: [0.0, 1.0],
        extend: [true; 2],
    };
    parsed.bbox = None;
    let parts = parts(&parsed, true);
    let mut tight = Meter::new(4096, "shading test");
    assert_eq!(
        ShadingSource::new(
            &parts,
            Transform::IDENTITY,
            &context,
            Use::Operator,
            (&mut tight, &mut Meter::new(usize::MAX, "test functions"))
        )
        .unwrap_err()
        .kind,
        ErrorKind::Limit
    );
    let mut bytes = Meter::new(8 << 20, "shading test");
    bytes.charge(19).unwrap();
    let source = ShadingSource::new(
        &parts,
        Transform::IDENTITY,
        &context,
        Use::Operator,
        (&mut bytes, &mut Meter::new(usize::MAX, "test functions")),
    )
    .unwrap();
    assert!(source.aligned.as_ref().unwrap().pixels() > 64_000);
    assert_eq!(bytes.used(), 19 + source.bytes());

    // Enough work to inspect the constant-function case, but not to
    // populate the table: refusal precedes the large table allocation.
    let mut bytes = Meter::new(4096, "table memory");
    let mut functions = Meter::new(1, "table function work");
    let error = ShadingSource::new(
        &parts,
        Transform::IDENTITY,
        &context,
        Use::Operator,
        (&mut bytes, &mut functions),
    )
    .unwrap_err();
    assert_eq!(error.message, "table function work");
    assert!(bytes.used() < bytes.max());
}

#[test]
fn solid_conversion_reserves_its_live_lookup_and_output_together() {
    let target = Target::new(Process::Rgb);
    let (composite, store, limits) = (Composite::default(), Store::new(1 << 20), Limits::default());
    let context = context(&target, &composite, &store, &limits);
    let paint = Paint::Solid(Color {
        space: Arc::new(ColorSpace::Indexed(Arc::new(pagura_color::Indexed {
            base: Arc::new(ColorSpace::DeviceRgb),
            high: 255,
            lookup: vec![128; 256 * 3].into(),
        }))),
        components: Components::from_slice(&[7.0]),
    });
    let lookup = 256 * 3 * size_of::<f64>();
    let mut functions = Meter::new(0, "solid function work");
    let tight = Meter::new(lookup, "solid memory");
    let error = Source::prepare(&paint, &context, 1, (&mut functions, &tight)).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
    let mut enough = Meter::new(17 + lookup + 3 * size_of::<f64>(), "solid memory");
    enough.charge(17).unwrap();
    let source = Source::prepare(&paint, &context, 1, (&mut functions, &enough)).unwrap();
    let (color, _) = source.constant().unwrap();
    assert_eq!(color, &[128.0 / 255.0; 3]);
    assert_eq!(source.bytes(), 3 * size_of::<f64>());
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
                let mut source = ShadingSource::new(
                    &parts(&parsed, false),
                    transform,
                    &context,
                    Use::Operator,
                    (
                        &mut Meter::new(limits.render_bytes, "test bytes"),
                        &mut Meter::new(usize::MAX, "test functions"),
                    ),
                )
                .unwrap();
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
                    source.span(0, y, 512, Precision::Exact, &mut out);
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

/// A column table holds only the columns of the region a mark paints in,
/// and spans inside and outside it take the colours an unclipped table
/// gives, bit for bit.
#[test]
fn aligned_tables_hold_the_painted_columns() {
    let target = Target::new(Process::Rgb);
    let (composite, store, limits) = (Composite::default(), Store::new(1 << 20), Limits::default());
    let wide = context(&target, &composite, &store, &limits);
    let narrow = PaintContext {
        region: DeviceRect {
            x0: 300,
            y0: 0,
            x1: 340,
            y1: 64,
        },
        ..context(&target, &composite, &store, &limits)
    };
    let mut rng = Rng(0xc0_1e);
    let mut tables = 0;
    for case in 0..40 {
        let mut parsed = shading(&mut rng);
        parsed.kind = ShadingKind::Axial {
            start: Point::new(rng.quarter(0.0, 200.0), 0.0),
            end: Point::new(rng.quarter(400.0, 900.0), 0.0),
            domain: [0.0, 1.0],
            extend: [rng.below(2) == 0, rng.below(2) == 0],
        };
        parsed.bbox = None;
        parsed.background = None;
        let parts = parts(&parsed, false);
        let source = |context: &PaintContext<'_>| {
            let mut bytes = Meter::new(8 << 20, "shading test");
            let mut functions = Meter::new(usize::MAX, "test functions");
            ShadingSource::new(
                &parts,
                Transform::IDENTITY,
                context,
                Use::Operator,
                (&mut bytes, &mut functions),
            )
            .unwrap()
        };
        let (all, some) = (source(&wide), source(&narrow));
        let (Some(full), Some(clipped)) = (&all.aligned, &some.aligned) else {
            continue;
        };
        if full.pixels() > 2 {
            tables += 1;
            assert!(
                clipped.pixels() <= 2 + 40,
                "case {case}: {} pixels",
                clipped.pixels()
            );
        }
        let (mut want, mut got) = (SpanColors::default(), SpanColors::default());
        for (x, len) in [(0, 1000), (290, 60), (320, 5)] {
            for precision in [Precision::Exact, Precision::Level] {
                all.span(x, 7, len, precision, &mut want);
                some.span(x, 7, len, precision, &mut got);
                assert_eq!(
                    want.colors.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                    got.colors.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                    "case {case} span {x}+{len}"
                );
                assert_eq!(want.alpha, got.alpha, "case {case} span {x}+{len}");
            }
        }
    }
    assert!(tables > 10, "{tables} tables");
}

/// An ICCBased RGB space through the sampled, sRGB-like profile of the
/// colour fixtures, whose conversion ends in the transfer function's power.
fn icc_rgb() -> ColorSpace {
    let bytes = pagura_testkit::fixture("color", "sampled-rgb.icc");
    let profile = pagura_color::Profile::parse(&bytes, 3, &Limits::default())
        .unwrap()
        .unwrap();
    ColorSpace::Icc(Arc::new(pagura_color::IccSpace {
        profile: Some(Arc::new(profile)),
        alternate: Arc::new(ColorSpace::DeviceRgb),
        components: 3,
        range: [[0.0, 1.0]; 4],
        object: None,
    }))
}

/// Rows of axial shadings kept to F32 levels approximate their colours
/// within [`pagura_color::APPROXIMATE`] of the exact ones and miss and
/// extend exactly as exact rows do: for steep and discontinuous stitching
/// (bounds a hair apart), few samples, long rows and placements far from
/// the origin.
#[test]
fn approximated_rows_stay_within_their_bound() {
    let mut rng = Rng(0x0a0e_77ed);
    let target = Target::new(Process::Rgb);
    let (composite, store, limits) = (Composite::default(), Store::new(1 << 20), Limits::default());
    let context = context(&target, &composite, &store, &limits);
    let icc = icc_rgb();
    let (mut approximated, mut values) = (0, 0);
    let before = APPROXIMATED.with(std::cell::Cell::get);
    for case in 0..200 {
        let pieces = (0..3).map(|_| function(&mut rng, 3)).collect::<Vec<_>>();
        let gap = [1e-9, 1e-4, 0.25][rng.below(3) as usize];
        let bound = rng.quarter(0.25, 0.5);
        let stitched = Function::stitching(StitchingFunction {
            domain: [0.0, 1.0],
            range: None,
            functions: pieces,
            bounds: vec![bound, bound + gap],
            encode: vec![[0.0, 1.0]; 3],
        })
        .unwrap();
        let start = Point::new(rng.quarter(0.0, 96.0), rng.quarter(0.0, 96.0));
        let end = Point::new(
            start.x + rng.quarter(-96.0, 96.0) + 0.25,
            start.y + rng.quarter(-96.0, 96.0),
        );
        let space = if rng.below(2) == 0 {
            icc.clone()
        } else {
            ColorSpace::DeviceRgb
        };
        let parsed = Parsed {
            name: String::from("stress"),
            space: Arc::new(space),
            function: Some(ShadingFunction::Single(Arc::new(stitched))),
            kind: ShadingKind::Axial {
                start,
                end,
                domain: [0.0, 1.0],
                extend: [rng.below(2) == 0, rng.below(2) == 0],
            },
            bbox: None,
            antialias: false,
            background: None,
            old_mesh: Vec::new(),
        };
        // Rotated, so no aligned table, and far from the origin.
        let far = [0.0, 1e3, 1e5][rng.below(3) as usize];
        let s = [8.0, 16.0][rng.below(2) as usize];
        let transform = Transform::new(0.8 * s, 0.6 * s, -0.6 * s, 0.8 * s, far, far);
        let source = prepared(&parts(&parsed, false), transform, &context, Use::Operator);
        let (mut exact, mut kept) = (SpanColors::default(), SpanColors::default());
        for row in 0..6 {
            let y = (far as u32).saturating_add(row * 300);
            let x0 = (far as u32).saturating_sub(512);
            source.span(x0, y, 2048, Precision::Exact, &mut exact);
            source.span(x0, y, 2048, Precision::Level, &mut kept);
            let mut tile = SpanColors::default();
            for first in (0..2048).step_by(37) {
                let len = 37.min(2048 - first);
                source.span(x0 + first as u32, y, len, Precision::Level, &mut tile);
                for (c, (&whole, &part)) in kept.colors[first * 3..(first + len) * 3]
                    .iter()
                    .zip(&tile.colors)
                    .enumerate()
                {
                    assert_eq!(
                        crate::kernel::level(whole).to_bits(),
                        crate::kernel::level(part).to_bits(),
                        "case {case} row {y} column {first} channel {c}: partition"
                    );
                }
            }
            assert_eq!(kept.alpha, exact.alpha, "case {case} row {y}: alpha");
            for (i, (&k, &e)) in kept.colors.iter().zip(&exact.colors).enumerate() {
                let level = |v: f64| crate::kernel::level(v).to_bits();
                let same = level(k) == level(e);
                assert!(
                    same || (k - e).abs() <= pagura_color::APPROXIMATE,
                    "case {case} row {y} value {i}: {k} against {e}"
                );
                assert!(same, "case {case} row {y} value {i}: ramp admission");
                approximated += usize::from(!same);
                values += 1;
            }
        }
    }
    // Many pixels were approximated (stitching of unsupported exponents and
    // extended stretches stay exact), and every stored level remains exact.
    let pixels = APPROXIMATED.with(std::cell::Cell::get) - before;
    assert!(
        pixels > 100_000 && approximated == 0,
        "{pixels} approximated pixels, {approximated} of {values} values moved"
    );
}

/// Rows of axial shadings whose samples all hit write the F32 levels of
/// the colours their spans give, bit for bit, as a store at full opacity
/// does; rows where a sample may miss write nothing.
#[test]
fn rows_write_the_levels_of_their_colours() {
    let mut rng = Rng(0x1e7e_15ed);
    let target = Target::new(Process::Rgb);
    let (composite, store, limits) = (Composite::default(), Store::new(1 << 20), Limits::default());
    let context = context(&target, &composite, &store, &limits);
    let icc = icc_rgb();
    let (mut written, mut declined) = (0, 0);
    for case in 0..120 {
        let pieces = (0..3).map(|_| function(&mut rng, 3)).collect::<Vec<_>>();
        let bound = rng.quarter(0.25, 0.5);
        let stitched = Function::stitching(StitchingFunction {
            domain: [0.0, 1.0],
            range: None,
            functions: pieces,
            bounds: vec![bound, bound + 1e-4],
            encode: vec![[0.0, 1.0]; 3],
        })
        .unwrap();
        let start = Point::new(rng.quarter(0.0, 96.0), rng.quarter(0.0, 96.0));
        let end = Point::new(
            start.x + rng.quarter(-96.0, 96.0) + 0.25,
            start.y + rng.quarter(-96.0, 96.0),
        );
        let space = if rng.below(2) == 0 {
            icc.clone()
        } else {
            ColorSpace::DeviceRgb
        };
        let parsed = Parsed {
            name: String::from("levels"),
            space: Arc::new(space),
            function: Some(ShadingFunction::Single(Arc::new(stitched))),
            kind: ShadingKind::Axial {
                start,
                end,
                domain: [0.0, 1.0],
                extend: [rng.below(4) != 0, rng.below(4) != 0],
            },
            bbox: None,
            antialias: false,
            background: None,
            old_mesh: Vec::new(),
        };
        let s = [8.0, 16.0][rng.below(2) as usize];
        let transform = Transform::new(0.8 * s, 0.6 * s, -0.6 * s, 0.8 * s, 0.0, 0.0);
        let source = prepared(&parts(&parsed, false), transform, &context, Use::Operator);
        let mut kept = SpanColors::default();
        for row in 0..8_u32 {
            let (x0, y) = (u32::try_from(rng.below(512)).unwrap(), row * 160);
            let len = 1 + rng.below(1500) as usize;
            let mut levels = vec![[-1.0_f32; 4]; len];
            let direct = source.levels(x0, y, &mut levels);
            source.span(x0, y, len, Precision::Level, &mut kept);
            if !direct {
                declined += 1;
                assert!(
                    levels.iter().all(|&p| p == [-1.0; 4]),
                    "case {case}: written"
                );
                continue;
            }
            written += 1;
            assert!(
                kept.alpha.is_empty(),
                "case {case} row {y}: a sample misses"
            );
            for (k, pixel) in levels.iter().enumerate() {
                let want = crate::kernel::levels(&kept.colors[k * 3..(k + 1) * 3]);
                assert_eq!(
                    pixel.map(f32::to_bits),
                    want.map(f32::to_bits),
                    "case {case} row {y} pixel {k}"
                );
            }
        }
    }
    assert!(
        written > 300 && declined > 50,
        "{written} written, {declined} declined"
    );
}
