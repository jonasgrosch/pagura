//! Differential tests of function evaluation against the previous
//! evaluation (`oracle`), static validation of calculator programs, and the
//! defined results of faults.

mod calculator;
pub(crate) mod oracle;
pub(crate) mod random;

use super::*;
use oracle::Oracle;
use pagura_core::ErrorKind;
use random::Rng;

/// A new function and the previous evaluation of the same parameters.
pub(crate) struct Pair {
    pub(crate) new: Arc<Function>,
    pub(crate) old: Oracle,
}

/// Compares one evaluation: wherever the previous evaluation succeeded the
/// new one is bit-identical, and its work never exceeds `cost`.
pub(crate) fn compare(pair: &Pair, input: &[f64], context: &str) -> bool {
    let n = pair.new.outputs();
    let mut old = vec![0.0; n];
    let mut work = 0;
    let Ok(()) = pair.old.evaluate_into(input, &mut old, &mut work) else {
        // The previous evaluation failed; the new one is total.
        let mut new = vec![0.0; n];
        pair.new.evaluate(input, &mut new);
        assert!(new.iter().all(|v| v.is_finite()), "{context}");
        return false;
    };
    let mut new = vec![f64::NAN; n];
    pair.new.evaluate(input, &mut new);
    assert_eq!(
        new.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        old.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        "{context} at {input:?}: {new:?} != {old:?}"
    );
    assert!(
        work <= pair.new.cost(),
        "{context}: work {work} > cost {}",
        pair.new.cost()
    );
    let mut tint_work = 0;
    let tint = pair.old.evaluate_with_work(input, &mut tint_work).unwrap();
    assert_eq!(
        tint.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        old.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        "{context}"
    );
    assert!(tint_work <= pair.new.cost(), "{context}");
    true
}

/// Inputs that probe every domain: the bounds, beyond them, interior points
/// and random values.
fn inputs(rng: &mut Rng, domain: &[[f64; 2]], count: usize) -> Vec<Vec<f64>> {
    let mut result = Vec::new();
    for i in 0..count {
        let input = domain
            .iter()
            .map(|&[low, high]| match i % 7 {
                0 => low,
                1 => high,
                2 => low - 0.5 * (high - low) - 1.0,
                3 => high + 0.5 * (high - low) + 1.0,
                4 => low.midpoint(high),
                _ => rng.range(low, high),
            })
            .collect();
        result.push(input);
    }
    result
}

#[test]
fn random_functions_match_the_previous_evaluation_bit_for_bit() {
    let mut rng = Rng::new(0x5eed_f00d);
    let mut compared = 0;
    for case in 0..3000 {
        let pair = random::function(&mut rng, 2);
        let domain = pair.new.domain().to_vec();
        for input in inputs(&mut rng, &domain, 24) {
            compared += usize::from(compare(&pair, &input, &format!("case {case}")));
        }
    }
    assert!(compared > 60_000, "{compared}");
}

#[test]
fn sampled_grid_points_and_byte_paths_match() {
    let mut rng = Rng::new(7);
    for case in 0..2000 {
        let pair = random::sampled(&mut rng);
        let domain = pair.new.domain().to_vec();
        // Inputs that land on grid points take the one-corner branches.
        for step in 0..=16 {
            let input: Vec<f64> = domain
                .iter()
                .map(|&[low, high]| low + (high - low) * f64::from(step) / 16.0)
                .collect();
            compare(&pair, &input, &format!("grid case {case}"));
        }
    }
}

#[test]
fn faults_have_defined_results() {
    // Inputs that are not finite read as made finite and clipped.
    let exponential = Function::exponential(ExponentialFunction {
        domain: [0.0, 2.0],
        range: None,
        c0: vec![0.0],
        c1: vec![1.0],
        exponent: 1.0,
    })
    .unwrap();
    let mut out = [0.0];
    for (input, expected) in [(f64::NAN, 0.0), (f64::INFINITY, 2.0), (-f64::INFINITY, 0.0)] {
        exponential.evaluate(&[input], &mut out);
        assert_eq!(out, [expected], "{input}");
    }
    // Missing inputs read as the lower bound; extra outputs are zeroed.
    let mut out = [9.0, 9.0];
    exponential.evaluate(&[], &mut out);
    assert_eq!(out, [0.0, 0.0]);
    // An overflowing power saturates instead of failing.
    let large = Function::exponential(ExponentialFunction {
        domain: [0.0, 1e300],
        range: None,
        c0: vec![0.0, 1.0],
        c1: vec![1.0, 1.0],
        exponent: 2.0,
    })
    .unwrap();
    let mut out = [0.0; 2];
    large.evaluate(&[1e300], &mut out);
    assert_eq!(out, [f64::MAX, 0.0]);
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one table of every constructor case, checked together"
)]
fn constructors_keep_the_previous_validation() {
    let exponential = |domain, exponent, c1: Vec<f64>| {
        Function::exponential(ExponentialFunction {
            domain,
            range: None,
            c0: vec![0.0],
            c1,
            exponent,
        })
    };
    for (domain, exponent) in [([-1.0, 1.0], 0.5), ([-1.0, 1.0], -1.0), ([0.0, 1.0], -2.0)] {
        assert_eq!(
            exponential(domain, exponent, vec![1.0]).unwrap_err().kind,
            ErrorKind::Syntax
        );
    }
    assert!(exponential([-1.0, 1.0], 2.0, vec![1.0]).is_ok());
    assert!(exponential([1.0, 2.0], -0.5, vec![1.0]).is_ok());
    assert_eq!(
        exponential([0.0, 1.0], 1.0, vec![1.0, 1.0])
            .unwrap_err()
            .message,
        "wrong exponential function output dimension"
    );
    assert_eq!(
        exponential([1.0, 0.0], 1.0, vec![1.0]).unwrap_err().message,
        "reversed function range"
    );
    let sampled = |size: Vec<u32>, bits, order, samples: &[u8]| {
        Function::sampled(
            SampledFunction {
                domain: vec![[0.0, 1.0]; size.len()],
                range: vec![[0.0, 1.0]],
                size,
                bits_per_sample: bits,
                order,
                encode: None,
                decode: None,
                samples: samples.into(),
            },
            &Limits::default(),
        )
    };
    assert!(sampled(vec![2], 8, 1, &[0, 255]).is_ok());
    for (size, bits, order, samples, message) in [
        (vec![2], 8, 1, &[0][..], "truncated sampled function"),
        (vec![0], 8, 1, &[0, 0][..], "invalid sampled function size"),
        (vec![2], 7, 1, &[0, 0][..], "invalid function BitsPerSample"),
        (
            vec![2],
            8,
            2,
            &[0, 0][..],
            "invalid sampled interpolation order",
        ),
    ] {
        assert_eq!(
            sampled(size, bits, order, samples).unwrap_err().message,
            message
        );
    }
    let limited = Limits {
        decoded_bytes: 1,
        ..Limits::default()
    };
    let error = Function::sampled(
        SampledFunction {
            domain: vec![[0.0, 1.0]],
            range: vec![[0.0, 1.0]],
            size: vec![2],
            bits_per_sample: 8,
            order: 1,
            encode: None,
            decode: None,
            samples: vec![0; 2].into(),
        },
        &limited,
    )
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
    let leaf = Arc::new(exponential([0.0, 1.0], 1.0, vec![1.0]).unwrap());
    let stitching = |bounds: Vec<f64>, encode: Vec<[f64; 2]>, domain| {
        Function::stitching(StitchingFunction {
            domain,
            range: None,
            functions: vec![Arc::clone(&leaf); 2],
            bounds,
            encode,
        })
    };
    assert!(stitching(vec![0.5], vec![[0.0, 1.0]; 2], [0.0, 1.0]).is_ok());
    for (bounds, encode, domain, message) in [
        (
            vec![],
            vec![[0.0, 1.0]; 2],
            [0.0, 1.0],
            "wrong stitching bounds or encode count",
        ),
        (
            vec![2.0],
            vec![[0.0, 1.0]; 2],
            [0.0, 1.0],
            "stitching bounds outside domain or unordered",
        ),
        (
            vec![0.5],
            vec![[0.0, 1.0]; 2],
            [0.5, 0.5],
            "stitching bounds outside domain or unordered",
        ),
    ] {
        assert_eq!(
            stitching(bounds, encode, domain).unwrap_err().message,
            message
        );
    }
}

#[test]
fn sampled_cost_counts_every_corner_and_is_bounded_by_function_work() {
    // Thirty-two singleton inputs: one corner of 33 units, plus the call.
    let parameters = SampledFunction {
        domain: vec![[0.0, 1.0]; 32],
        range: vec![[0.0, 1.0]],
        size: vec![1; 32],
        bits_per_sample: 8,
        order: 3,
        encode: None,
        decode: None,
        samples: vec![128].into(),
    };
    let function = Function::sampled(parameters.clone(), &Limits::default()).unwrap();
    assert_eq!(function.cost(), 34);
    let mut out = [0.0];
    function.evaluate(&[0.0; 32], &mut out);
    assert_eq!(out, [128.0 / 255.0]);
    let limits = Limits {
        function_work: 33,
        ..Limits::default()
    };
    assert_eq!(
        Function::sampled(parameters, &limits).unwrap_err().kind,
        ErrorKind::Limit
    );
}

#[test]
fn constant_functions_are_recognized_exactly() {
    let mut rng = Rng::new(99);
    let mut found = 0;
    for _ in 0..2000 {
        let pair = random::function(&mut rng, 2);
        let n = pair.new.outputs();
        let mut constant = vec![0.0; n];
        if !pair.new.constant(&mut constant) {
            continue;
        }
        found += 1;
        let domain = pair.new.domain().to_vec();
        for input in inputs(&mut rng, &domain, 32) {
            let mut out = vec![0.0; n];
            pair.new.evaluate(&input, &mut out);
            assert_eq!(
                out.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                constant.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
            );
        }
    }
    assert!(found > 50, "{found}");
}
