//! Certified ramps against exact evaluation over random one-input stages.

use super::*;
use crate::convert::{Conversion, Process, Target};
use crate::function::tests::random::{self, Rng};
use crate::icc::Profile;
use crate::space::{ColorSpace, IccSpace};
use pagura_core::Limits;

/// An F32 level's bits: `255·v` rounded to odd single precision, which
/// tells apart any two values a nearer rounding could merge.
fn level(value: f64) -> u32 {
    let value = value.clamp(0.0, 1.0) * 255.0;
    #[expect(clippy::cast_possible_truncation, reason = "the rounding under test")]
    let nearest = value as f32;
    let truncated = nearest.to_bits() - u32::from(f64::from(nearest) > value);
    truncated | u32::from(f64::from(nearest) != value)
}

/// A colour space of `components` components into which a random function
/// feeds, with its target.
fn space(rng: &mut Rng) -> (ColorSpace, Target) {
    let profile = || {
        let bytes = pagura_testkit::fixture("color", "sampled-rgb.icc");
        Arc::new(
            Profile::parse(&bytes, 3, &Limits::default())
                .unwrap()
                .unwrap(),
        )
    };
    let space = match rng.below(5) {
        0 => ColorSpace::DeviceGray,
        1 => ColorSpace::DeviceRgb,
        2 => ColorSpace::DeviceCmyk,
        _ => ColorSpace::Icc(Arc::new(IccSpace {
            profile: Some(profile()),
            alternate: Arc::new(ColorSpace::DeviceRgb),
            components: 3,
            range: [[0.0, 1.0]; 4],
            object: None,
        })),
    };
    let process = if rng.chance(25) {
        Process::Gray
    } else {
        Process::Rgb
    };
    (space, Target::new(process))
}

/// A random one-input function of `outputs` outputs.
fn function(rng: &mut Rng, outputs: usize) -> Arc<Function> {
    loop {
        let pair = match rng.below(3) {
            0 => random::exponential(rng, outputs),
            1 => random::sampled_with(rng, 1, outputs),
            _ => random::stitching(rng, 2),
        };
        if pair.new.outputs() == outputs {
            return pair.new;
        }
    }
}

/// The exact conversion of `stage` at `x`.
fn exact(stage: RampStage<'_>, converter: &Converter, components: usize, x: f64) -> Vec<f64> {
    let mut values = vec![0.0; components];
    match stage {
        RampStage::Single(f) => f.evaluate(&[x], &mut values),
        RampStage::PerComponent(fs) => {
            for (f, value) in fs.iter().zip(&mut values) {
                f.evaluate(&[x], std::slice::from_mut(value));
            }
        }
    }
    let mut out = vec![0.0; converter.target().channels()];
    converter.convert(&values, &mut out);
    out
}

#[test]
fn certified_colours_round_as_exact_colours() {
    let mut rng = Rng::new(0x5eed_c0102);
    let (mut samples, mut certified, mut ramps) = (0_usize, 0_usize, 0_usize);
    for case in 0..400 {
        let (space, target) = space(&mut rng);
        let components = space.components();
        let converter = Converter::new(&space, &target, &Conversion::default()).unwrap();
        let single;
        let each: Vec<Arc<Function>>;
        let stage = if rng.chance(70) {
            single = function(&mut rng, components);
            RampStage::Single(&single)
        } else {
            each = (0..components).map(|_| function(&mut rng, 1)).collect();
            RampStage::PerComponent(&each)
        };
        let domain = match stage {
            RampStage::Single(f) => f.domain()[0],
            RampStage::PerComponent(fs) => fs[0].domain()[0],
        };
        // A shading's inputs span its function's domain; wider inputs clip.
        let wide = rng.chance(25);
        let inputs = if wide {
            [domain[0] - 0.25, domain[1] + 0.25]
        } else {
            domain
        };
        let ramp = Ramp::new(
            stage,
            components,
            &converter,
            (inputs, 2048),
            &mut Meter::new(usize::MAX, "ramp test"),
        );
        if ramp.is_empty() {
            continue;
        }
        ramps += 1;
        let mut out = vec![0.0; target.channels()];
        for i in 0..3000 {
            let x = match i % 4 {
                0 => inputs[0] + (inputs[1] - inputs[0]) * (f64::from(i) / 3000.0),
                1 => *[domain[0], domain[1], inputs[0], inputs[1]]
                    .get(rng.below(4))
                    .unwrap(),
                _ => rng.range(inputs[0], inputs[1]),
            };
            if !wide {
                samples += 1;
            }
            if !ramp.convert(x, &mut out, level) {
                continue;
            }
            certified += usize::from(!wide);
            let expected = exact(stage, &converter, components, x);
            for (c, (&got, &want)) in out.iter().zip(&expected).enumerate() {
                assert_eq!(
                    level(got),
                    level(want),
                    "case {case}, x {x}, channel {c}: {got} vs {want} ({stage:?})"
                );
            }
        }
    }
    // Many stages have enclosures, and across their domains most samples
    // are certified.
    assert!(ramps > 120, "{ramps} ramps");
    assert!(certified * 10 > samples * 8, "{certified} of {samples}");
}

/// A ramp charges its storage before allocating it: the least budget that
/// builds one exceeds what it keeps by its temporaries (here a reversed
/// grid of 4,096 samples, which leaves almost every cell pending while it
/// cuts), one byte less builds none and returns every charge, and a built
/// ramp keeps exactly the bytes it holds charged.
#[test]
fn construction_charges_its_peak_storage() {
    let samples: Vec<u8> = (0..4096 * 3)
        .map(|i| u8::try_from(i % 251).unwrap())
        .collect();
    let function = Function::sampled(
        crate::function::SampledFunction {
            domain: vec![[0.0, 1.0]],
            range: vec![[0.0, 1.0]; 3],
            size: vec![4096],
            bits_per_sample: 8,
            order: 1,
            encode: Some(vec![[4095.0, 0.0]]),
            decode: None,
            samples: samples.into(),
        },
        &Limits::default(),
    )
    .unwrap();
    let converter = Converter::new(
        &ColorSpace::DeviceRgb,
        &Target::new(Process::Rgb),
        &Conversion::default(),
    )
    .unwrap();
    let build = |max: usize| {
        let mut bytes = Meter::new(max, "ramp test");
        let ramp = Ramp::new(
            RampStage::Single(&function),
            3,
            &converter,
            ([0.0, 1.0], 4096),
            &mut bytes,
        );
        (ramp, bytes.used())
    };
    let (ramp, used) = build(usize::MAX);
    assert!(ramp.len() > 4000, "{} pieces", ramp.len());
    assert_eq!(used, ramp.bytes());
    let (mut low, mut high) = (0, used * 4);
    while low + 1 < high {
        let mid = low + (high - low) / 2;
        if build(mid).0.is_empty() {
            low = mid;
        } else {
            high = mid;
        }
    }
    let (least, refused) = (build(high), build(low));
    // At the least budget a buffer may stay unshrunk; what it holds stays
    // charged.
    assert_eq!((least.0.len(), least.1), (ramp.len(), least.0.bytes()));
    assert!(
        refused.0.is_empty() && refused.1 == 0,
        "{} bytes kept",
        refused.1
    );
    // The pieces, terms and index alone, which is all an admission of the
    // kept bytes per piece covered, are not enough.
    let kept = 4096 * (size_of::<[f64; 2]>() + 3 * size_of::<Term>() + 4 * size_of::<u32>());
    assert!(high > kept, "{high} bytes built it, {kept} kept");
}

/// Every piece a row may approximate by writes, for an input within the
/// error of a sample, values within [`APPROXIMATE`] of the sample's exact
/// conversion; pieces whose sRGB power is bounded per sample approximate
/// by their wider cubic too.
#[test]
fn pieces_approximate_within_their_bound() {
    let mut rng = Rng::new(0xa991_0c5e);
    let (mut approximated, mut margins) = (0_usize, 0_usize);
    for case in 0..400 {
        let (space, target) = space(&mut rng);
        let components = space.components();
        let converter = Converter::new(&space, &target, &Conversion::default()).unwrap();
        let function = function(&mut rng, components);
        let stage = RampStage::Single(&function);
        let inputs = function.domain()[0];
        let ramp = Ramp::new(
            stage,
            components,
            &converter,
            (inputs, 2048),
            &mut Meter::new(usize::MAX, "ramp test"),
        );
        let error = [0.0, 1e-12, 1e-9][rng.below(3)];
        let mut out = vec![0.0; target.channels()];
        let mut hint = 0;
        for i in 0..2000 {
            let x = inputs[0] + (inputs[1] - inputs[0]) * (f64::from(i) / 2000.0);
            let Some(piece) = ramp.piece(x, error, &mut hint) else {
                continue;
            };
            let at = (x + rng.range(-error, error)).clamp(inputs[0], inputs[1]);
            piece.evaluate(at, &mut out);
            approximated += 1;
            let start = ramp
                .pieces
                .iter()
                .position(|&[low, high]| low <= x && x <= high)
                .unwrap();
            margins += usize::from(
                ramp.terms[start * ramp.channels..(start + 1) * ramp.channels]
                    .iter()
                    .any(|term| term.margin != 0.0),
            );
            let expected = exact(stage, &converter, components, x);
            for (c, (&got, &want)) in out.iter().zip(&expected).enumerate() {
                assert!(
                    (got - want).abs() <= APPROXIMATE,
                    "case {case}, x {x}, channel {c}: {got} vs {want} ({stage:?})"
                );
            }
        }
    }
    assert!(
        approximated > 100_000 && margins > 500,
        "{approximated} approximated, {margins} through powers bounded per sample"
    );
}
