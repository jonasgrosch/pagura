//! Random valid functions, built as new functions and oracle pairs.
#![expect(clippy::cast_possible_truncation, reason = "small random counts")]

use super::{Pair, oracle::Oracle};
use crate::function::{ExponentialFunction, Function, SampledFunction, StitchingFunction};
use pagura_core::Limits;
use std::sync::Arc;

/// A small deterministic generator (xorshift64*).
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    pub(crate) fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// A value in `0..n`.
    pub(crate) fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    pub(crate) fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1_u64 << 53) as f64
    }

    pub(crate) fn range(&mut self, low: f64, high: f64) -> f64 {
        low + (high - low) * self.unit()
    }

    pub(crate) fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }

    /// A "nice" or random number in `low..high`.
    fn number(&mut self, low: f64, high: f64) -> f64 {
        match self.below(4) {
            0 => (self.range(low, high) * 4.0).round() / 4.0,
            _ => self.range(low, high),
        }
    }

    fn interval(&mut self, low: f64, high: f64) -> [f64; 2] {
        let a = self.number(low, high);
        let b = self.number(low, high);
        if self.chance(10) {
            [a, a]
        } else {
            [a.min(b), a.max(b)]
        }
    }
}

/// A random function nested at most `depth` levels.
pub(crate) fn function(rng: &mut Rng, depth: usize) -> Pair {
    match rng.below(if depth > 0 { 3 } else { 2 }) {
        0 => {
            let outputs = 1 + rng.below(3);
            exponential(rng, outputs)
        }
        1 => sampled(rng),
        _ => stitching(rng, depth),
    }
}

pub(crate) fn exponential(rng: &mut Rng, outputs: usize) -> Pair {
    let exponent = match rng.below(5) {
        0 => 1.0,
        1 => 2.0,
        2 => rng.number(0.1, 4.0),
        3 => -rng.number(0.5, 3.0).round().max(1.0),
        _ => rng.number(-3.0, 3.0),
    };
    let domain = if exponent.fract() != 0.0 || exponent < 0.0 {
        let [a, b] = rng.interval(0.0, 3.0);
        if exponent < 0.0 {
            [a + 0.25, b + 0.25]
        } else {
            [a, b]
        }
    } else {
        rng.interval(-3.0, 3.0)
    };
    let c0: Vec<f64> = (0..outputs).map(|_| rng.number(-1.0, 2.0)).collect();
    let c1: Vec<f64> = if rng.chance(15) {
        c0.clone()
    } else {
        (0..outputs).map(|_| rng.number(-1.0, 2.0)).collect()
    };
    let range = rng
        .chance(50)
        .then(|| (0..outputs).map(|_| rng.interval(-0.5, 1.5)).collect());
    let parameters = ExponentialFunction {
        domain,
        range,
        c0,
        c1,
        exponent,
    };
    Pair {
        old: Oracle::exponential(&parameters),
        new: Arc::new(Function::exponential(parameters).unwrap()),
    }
}

pub(crate) fn sampled(rng: &mut Rng) -> Pair {
    let inputs = 1 + rng.below(3);
    let outputs = 1 + rng.below(4);
    sampled_with(rng, inputs, outputs)
}

pub(crate) fn sampled_with(rng: &mut Rng, inputs: usize, outputs: usize) -> Pair {
    let bits = [1, 2, 4, 8, 8, 8, 12, 16, 24, 32][rng.below(10)];
    let size: Vec<u32> = (0..inputs).map(|_| 1 + rng.below(6) as u32).collect();
    let domain: Vec<[f64; 2]> = (0..inputs)
        .map(|_| {
            if rng.chance(40) {
                [0.0, 1.0]
            } else {
                rng.interval(-2.0, 2.0)
            }
        })
        .collect();
    let range: Vec<[f64; 2]> = (0..outputs)
        .map(|_| {
            if rng.chance(50) {
                [0.0, 1.0]
            } else {
                rng.interval(-1.0, 2.0)
            }
        })
        .collect();
    let encode = rng.chance(40).then(|| {
        size.iter()
            .map(|&n| {
                let last = f64::from(n - 1);
                match rng.below(3) {
                    0 => [last, 0.0],
                    1 => [0.0, last],
                    _ => [rng.number(-1.0, last + 1.0), rng.number(-1.0, last + 1.0)],
                }
            })
            .collect()
    });
    let decode = rng.chance(40).then(|| {
        (0..outputs)
            .map(|_| [rng.number(-1.0, 2.0), rng.number(-1.0, 2.0)])
            .collect()
    });
    let count = outputs * size.iter().map(|&n| n as usize).product::<usize>();
    let bytes = (count * bits).div_ceil(8) + rng.below(3);
    let samples: Arc<[u8]> = (0..bytes).map(|_| rng.next() as u8).collect();
    let parameters = SampledFunction {
        domain,
        range,
        size,
        bits_per_sample: bits as u8,
        order: if rng.chance(50) { 3 } else { 1 },
        encode,
        decode,
        samples,
    };
    Pair {
        old: Oracle::sampled(&parameters),
        new: Arc::new(Function::sampled(parameters, &Limits::default()).unwrap()),
    }
}

pub(crate) fn stitching(rng: &mut Rng, depth: usize) -> Pair {
    let count = 1 + rng.below(4);
    let outputs = 1 + rng.below(3);
    let mut domain = rng.interval(-2.0, 2.0);
    if count > 1 && domain[0] == domain[1] {
        domain[1] += 1.0;
    }
    let mut bounds: Vec<f64> = (1..count)
        .map(|_| rng.number(domain[0], domain[1]).clamp(domain[0], domain[1]))
        .collect();
    bounds.sort_by(f64::total_cmp);
    bounds.dedup();
    if rng.chance(20)
        && let Some(first) = bounds.first_mut()
    {
        *first = domain[0];
    }
    let count = bounds.len() + 1;
    let mut new = Vec::new();
    let mut old = Vec::new();
    for _ in 0..count {
        let pair = match rng.below(if depth > 1 { 3 } else { 2 }) {
            0 => exponential(rng, outputs),
            1 => sampled_with(rng, 1, outputs),
            _ => {
                let mut pair = stitching(rng, depth - 1);
                while pair.new.outputs() != outputs {
                    pair = stitching(rng, depth - 1);
                }
                pair
            }
        };
        new.push(pair.new);
        old.push(pair.old);
    }
    let encode: Vec<[f64; 2]> = (0..count)
        .map(|_| [rng.number(-0.5, 1.5), rng.number(-0.5, 1.5)])
        .collect();
    let range = rng.chance(40).then(|| {
        (0..outputs)
            .map(|_| rng.interval(-0.5, 1.5))
            .collect::<Vec<_>>()
    });
    Pair {
        old: Oracle::stitching(domain, range.clone(), old, bounds.clone(), encode.clone()),
        new: Arc::new(
            Function::stitching(StitchingFunction {
                domain,
                range,
                functions: new,
                bounds,
                encode,
            })
            .unwrap(),
        ),
    }
}
