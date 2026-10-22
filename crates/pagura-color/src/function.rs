//! PDF functions (7.10) without PDF objects. The interpreter reads function
//! dictionaries and streams into the parameter structs here; construction
//! validates them, and evaluation is then total: it cannot fail and does
//! not allocate (ADR 0034).
//!
//! # Defined results
//!
//! Where evaluation used to fail, it now has a defined result. These
//! cases never arise from well-formed functions evaluated inside their
//! domain, and results are identical to the previous evaluation wherever
//! that one succeeded.
//!
//! - Every input and every real result is made finite first: NaN becomes 0
//!   and ±∞ become ±[`f64::MAX`]. Inputs are then clipped to the domain and
//!   outputs to the range.
//! - A missing input reads as its domain's lower bound; extra inputs are
//!   ignored.
//! - Type 4 calculator programs are validated statically, so stack and
//!   operand-kind faults cannot occur. Arithmetic faults follow the rule
//!   above: `1 0 div` is [`f64::MAX`], `-1 sqrt` and `0 0 div` are 0, `0 ln`
//!   is −[`f64::MAX`] and `0 0 atan` is 0. `idiv` and `mod` by zero give 0,
//!   `−2147483648 −1 idiv` gives 2147483647 and `cvi` saturates to the
//!   integer range. An integer result that overflowed into a real (integer
//!   `add`, `sub`, `mul`, `abs` and `neg` do) is truncated and saturated
//!   where an integer is required.

mod calculator;
mod sampled;

use crate::space::MAX_COMPONENTS;
use pagura_core::{Error, Limits, Result};
use std::sync::Arc;

/// A validated function of `m` inputs and `n` outputs.
#[derive(Debug, Clone)]
pub struct Function {
    domain: Box<[[f64; 2]]>,
    range: Option<Box<[[f64; 2]]>>,
    outputs: usize,
    cost: usize,
    kind: Kind,
}

#[derive(Debug, Clone)]
enum Kind {
    Sampled(Box<sampled::Sampled>),
    Exponential {
        c0: Box<[f64]>,
        /// `C1 − C0`, as the interpolation computes it.
        delta: Box<[f64]>,
        exponent: f64,
    },
    Stitching(Stitching),
    Calculator(calculator::Program),
}

#[derive(Debug, Clone)]
struct Stitching {
    functions: Box<[Arc<Function>]>,
    bounds: Box<[f64]>,
    encode: Box<[[f64; 2]]>,
}

/// Parameters of a Type 0 sampled function (7.10.2).
///
/// A plain record of the dictionary's entries, built with a struct literal,
/// so it is not `#[non_exhaustive]`.
#[derive(Debug, Clone)]
pub struct SampledFunction {
    /// `Domain`, one interval per input.
    pub domain: Vec<[f64; 2]>,
    /// `Range`, one interval per output.
    pub range: Vec<[f64; 2]>,
    /// `Size`, samples per input dimension.
    pub size: Vec<u32>,
    /// `BitsPerSample`: 1, 2, 4, 8, 12, 16, 24 or 32.
    pub bits_per_sample: u8,
    /// `Order`: 1 (linear) or 3 (cubic spline).
    pub order: u8,
    /// `Encode`, if present.
    pub encode: Option<Vec<[f64; 2]>>,
    /// `Decode`, if present.
    pub decode: Option<Vec<[f64; 2]>>,
    /// The decoded stream data.
    pub samples: Arc<[u8]>,
}

/// Parameters of a Type 2 exponential interpolation function (7.10.3).
///
/// A plain record of the dictionary's entries (with the defaults `C0 = [0]`
/// and `C1 = [1]` filled in), so it is not `#[non_exhaustive]`.
#[derive(Debug, Clone)]
pub struct ExponentialFunction {
    /// `Domain`.
    pub domain: [f64; 2],
    /// `Range`, if present.
    pub range: Option<Vec<[f64; 2]>>,
    /// `C0`.
    pub c0: Vec<f64>,
    /// `C1`.
    pub c1: Vec<f64>,
    /// `N`.
    pub exponent: f64,
}

/// Parameters of a Type 3 stitching function (7.10.4).
///
/// A plain record of the dictionary's entries, so it is not
/// `#[non_exhaustive]`.
#[derive(Debug, Clone)]
pub struct StitchingFunction {
    /// `Domain`.
    pub domain: [f64; 2],
    /// `Range`, if present.
    pub range: Option<Vec<[f64; 2]>>,
    /// `Functions`, already validated.
    pub functions: Vec<Arc<Function>>,
    /// `Bounds`.
    pub bounds: Vec<f64>,
    /// `Encode`.
    pub encode: Vec<[f64; 2]>,
}

/// Parameters of a Type 4 PostScript calculator function (7.10.5).
///
/// A plain record of the dictionary's entries, so it is not
/// `#[non_exhaustive]`.
#[derive(Debug, Clone)]
pub struct CalculatorFunction {
    /// `Domain`.
    pub domain: Vec<[f64; 2]>,
    /// `Range`.
    pub range: Vec<[f64; 2]>,
    /// The program text (the decoded stream).
    pub program: Arc<[u8]>,
}

impl Function {
    /// A Type 0 sampled function.
    ///
    /// # Errors
    ///
    /// Returns `Syntax` for inconsistent parameters or short sample data, and
    /// a `Limit` error when the sample table exceeds `decoded_bytes`, when
    /// the function has more than [`MAX_COMPONENTS`] inputs, or when one
    /// evaluation would cost more than `function_work`.
    pub fn sampled(parameters: SampledFunction, limits: &Limits) -> Result<Self> {
        let domain = intervals(&parameters.domain)?;
        let range = intervals(&parameters.range)?;
        let outputs = dimensions(domain.len(), range.len())?;
        let unit = *domain == [[0.0, 1.0]];
        let sampled = sampled::Sampled::new(parameters, outputs, unit, limits)?;
        let cost = sampled.cost().saturating_add(1);
        if cost > limits.function_work {
            return Err(Error::limit("sampled interpolation corner budget exceeded"));
        }
        Ok(Self {
            domain,
            range: Some(range),
            outputs,
            cost,
            kind: Kind::Sampled(Box::new(sampled)),
        })
    }

    /// A Type 2 exponential interpolation function.
    ///
    /// # Errors
    ///
    /// Returns `Syntax` for inconsistent parameters, including a domain with
    /// inputs for which `x^N` is undefined.
    pub fn exponential(parameters: ExponentialFunction) -> Result<Self> {
        let ExponentialFunction {
            domain,
            range,
            c0,
            c1,
            exponent,
        } = parameters;
        let [min, max] = domain;
        let domain = intervals(&[domain])?;
        let range = range.as_deref().map(intervals).transpose()?;
        let outputs = dimensions(1, c0.len())?;
        finite(c0.iter().chain(&c1).copied().chain([exponent]))?;
        if (exponent.fract() != 0.0 && min < 0.0) || (exponent < 0.0 && min <= 0.0 && max >= 0.0) {
            return Err(Error::syntax(
                "exponential domain includes undefined inputs",
            ));
        }
        if c1.len() != outputs {
            return Err(Error::syntax("wrong exponential function output dimension"));
        }
        if range.as_ref().is_some_and(|r| r.len() != outputs) {
            return Err(Error::syntax("wrong function output dimension"));
        }
        let delta = c0.iter().zip(&c1).map(|(&low, &high)| high - low).collect();
        Ok(Self {
            domain,
            range,
            outputs,
            cost: 1,
            kind: Kind::Exponential {
                c0: c0.into(),
                delta,
                exponent,
            },
        })
    }

    /// A Type 3 stitching function.
    ///
    /// # Errors
    ///
    /// Returns `Syntax` for inconsistent parameters: subfunctions that do not
    /// all have one input and the same number of outputs, `Bounds` and
    /// `Encode` counts that do not match `Functions`, and bounds outside the
    /// domain or not increasing.
    pub fn stitching(parameters: StitchingFunction) -> Result<Self> {
        let StitchingFunction {
            domain,
            range,
            functions,
            bounds,
            encode,
        } = parameters;
        let [min, max] = domain;
        let domain = intervals(&[domain])?;
        let range = range.as_deref().map(intervals).transpose()?;
        let outputs = functions
            .first()
            .ok_or(Error::syntax("stitching function requires Functions"))?
            .outputs();
        if functions.iter().any(|f| f.inputs() != 1) {
            return Err(Error::syntax("wrong function input dimension"));
        }
        if functions.iter().any(|f| f.outputs() != outputs)
            || range.as_ref().is_some_and(|r| r.len() != outputs)
        {
            return Err(Error::syntax("wrong function output dimension"));
        }
        if bounds.len() + 1 != functions.len() || encode.len() != functions.len() {
            return Err(Error::syntax("wrong stitching bounds or encode count"));
        }
        finite(bounds.iter().chain(encode.iter().flatten()).copied())?;
        if (functions.len() > 1 && min == max)
            || bounds.iter().any(|&v| v < min || v > max)
            || bounds.windows(2).any(|v| v[0] >= v[1])
        {
            return Err(Error::syntax(
                "stitching bounds outside domain or unordered",
            ));
        }
        let cost = functions
            .iter()
            .map(|f| f.cost())
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        Ok(Self {
            domain,
            range,
            outputs,
            cost,
            kind: Kind::Stitching(Stitching {
                functions: functions.into(),
                bounds: bounds.into(),
                encode: encode.into(),
            }),
        })
    }

    /// A Type 4 calculator function, parsed and validated statically: every
    /// branch's stack effect and operand kinds are checked, so evaluation
    /// cannot fail, and arithmetic faults have defined results (see the
    /// module documentation).
    ///
    /// The operand stack holds at most [`Limits::CALCULATOR_STACK`] entries
    /// (Annex C). The counts of `copy`, `index` and `roll` must be known
    /// before evaluation (integer literals, or integer arithmetic on them),
    /// and both branches of `ifelse` (or the branch of `if` and its absence)
    /// must leave the same stack depth and operand kinds (boolean or number).
    ///
    /// # Errors
    ///
    /// Returns `Syntax` for a malformed program (unknown operators, invalid
    /// numbers, unbalanced braces, stack underflow, operands of the wrong
    /// kind, a result count other than the range's), `Unsupported` for a
    /// program whose stack effect or operand kinds depend on its data and so
    /// cannot be validated, and a `Limit` error for a program nested deeper
    /// than `depth`, with more than `value_nodes` instructions, or deeper
    /// than the operand stack. Positions are
    /// [`Position::Buffer`](pagura_core::Position) offsets in the program.
    pub fn calculator(parameters: CalculatorFunction, limits: &Limits) -> Result<Self> {
        let CalculatorFunction {
            domain,
            range,
            program,
        } = parameters;
        let domain = intervals(&domain)?;
        let range = intervals(&range)?;
        let outputs = dimensions(domain.len(), range.len())?;
        let program = calculator::Program::new(&program, domain.len(), outputs, limits)?;
        Ok(Self {
            domain,
            range: Some(range),
            outputs,
            cost: program.cost().saturating_add(1),
            kind: Kind::Calculator(program),
        })
    }

    /// The number of inputs `m`.
    pub fn inputs(&self) -> usize {
        self.domain.len()
    }

    /// The number of outputs `n`.
    pub fn outputs(&self) -> usize {
        self.outputs
    }

    /// The domain, one interval per input.
    pub fn domain(&self) -> &[[f64; 2]] {
        &self.domain
    }

    /// The `function_work` units one evaluation costs at most (ADR 0016),
    /// counted as evaluation always has: one per function evaluated
    /// (subfunctions of a stitching function included), one per calculator
    /// instruction executed and per value `copy` or `roll` moves, and
    /// `m + n` per sample tuple a sampled function interpolates.
    pub fn cost(&self) -> usize {
        self.cost
    }

    /// Fills `output` as [`Self::evaluate`] does and returns true when the
    /// result provably does not depend on the input, bit for bit: an
    /// exponential function with `C0 = C1` (no `-0.0` among them) whose
    /// power stays finite over the domain, and a stitching function whose
    /// subfunctions are all such constants with identical outputs.
    pub fn constant(&self, output: &mut [f64]) -> bool {
        let count = output.len().min(self.outputs);
        let (values, rest) = output.split_at_mut(count);
        rest.fill(0.0);
        let constant = match &self.kind {
            Kind::Exponential {
                c0,
                delta,
                exponent,
            } => {
                // `low + x · 0` is `low` exactly when `x` is finite and `low`
                // is not -0.0 (whose sum with +0.0 is +0.0).
                let [min, max] = self.domain.first().copied().unwrap_or([0.0; 2]);
                let largest = if *exponent < 0.0 {
                    min.abs().min(max.abs())
                } else {
                    min.abs().max(max.abs())
                };
                let constant = delta.iter().all(|&d| d == 0.0)
                    && c0.iter().all(|&v| v != 0.0 || v.is_sign_positive())
                    && largest.powf(*exponent).is_finite();
                for (out, &low) in values.iter_mut().zip(c0) {
                    *out = low;
                }
                constant
            }
            Kind::Stitching(stitching) => stitching.constant(values),
            Kind::Sampled(_) | Kind::Calculator(_) => false,
        };
        if constant {
            self.clip(values);
        }
        constant
    }

    /// Evaluates the function: inputs are clipped to the domain and outputs to
    /// the range. `output` should hold [`Self::outputs`] values: only that
    /// many are written, and any further values are set to 0. Extra inputs
    /// are ignored and missing ones read as the domain's lower bound.
    pub fn evaluate(&self, input: &[f64], output: &mut [f64]) {
        let count = output.len().min(self.outputs);
        let (values, rest) = output.split_at_mut(count);
        rest.fill(0.0);
        match &self.kind {
            Kind::Exponential {
                c0,
                delta,
                exponent,
            } => {
                let x = self.input(input, 0).powf(*exponent);
                for ((out, &low), &delta) in values.iter_mut().zip(c0).zip(delta) {
                    *out = low + x * delta;
                }
            }
            Kind::Stitching(stitching) => self.stitch(input, stitching, values),
            Kind::Sampled(sampled) => sampled.evaluate(|i| self.input(input, i), values),
            Kind::Calculator(program) => {
                let mut clipped = [0.0; MAX_COMPONENTS];
                for (i, value) in clipped.iter_mut().enumerate().take(self.inputs()) {
                    *value = self.input(input, i);
                }
                program.evaluate(clipped.get(..self.inputs()).unwrap_or(&[]), values);
            }
        }
        self.clip(values);
    }

    /// Input `i` made finite and clipped to its domain interval.
    fn input(&self, input: &[f64], i: usize) -> f64 {
        let [low, high] = self.domain.get(i).copied().unwrap_or([0.0; 2]);
        input
            .get(i)
            .map_or(low, |&value| make_finite(value).clamp(low, high))
    }

    /// Makes outputs finite and clips them to the range.
    fn clip(&self, values: &mut [f64]) {
        for (i, value) in values.iter_mut().enumerate() {
            *value = make_finite(*value);
            if let Some([low, high]) = self.range.as_ref().and_then(|r| r.get(i)) {
                *value = value.clamp(*low, *high);
            }
        }
    }

    /// Evaluates the subfunction whose subdomain holds the input (7.10.4).
    fn stitch(&self, input: &[f64], stitching: &Stitching, values: &mut [f64]) {
        let Stitching {
            functions,
            bounds,
            encode,
        } = stitching;
        let x = self.input(input, 0);
        let [min, max] = self.domain.first().copied().unwrap_or([0.0; 2]);
        // The first degenerate interval owns its endpoint (7.10.4).
        let index = if bounds.first() == Some(&x) && min == x {
            0
        } else {
            bounds.partition_point(|&b| x >= b)
        };
        let low = index
            .checked_sub(1)
            .and_then(|i| bounds.get(i))
            .copied()
            .unwrap_or(min);
        let high = bounds.get(index).copied().unwrap_or(max);
        let encode = encode.get(index).copied().unwrap_or([0.0; 2]);
        if let Some(function) = functions.get(index) {
            function.evaluate(&[interpolate(x, [low, high], encode)], values);
        }
    }
}

impl Stitching {
    /// Whether every subfunction is a constant with the same outputs, which
    /// it writes to `values`.
    fn constant(&self, values: &mut [f64]) -> bool {
        let mut other = [0.0; MAX_COMPONENTS];
        let Some(other) = other.get_mut(..values.len()) else {
            return false;
        };
        let mut functions = self.functions.iter();
        if !functions.next().is_some_and(|f| f.constant(values)) {
            return false;
        }
        functions.all(|f| {
            f.constant(other)
                && values
                    .iter()
                    .zip(&*other)
                    .all(|(a, b)| a.to_bits() == b.to_bits())
        })
    }
}

/// Maps `value` linearly from `from` onto `to` (7.10.2, Interpolate); a
/// degenerate `from` maps to `to[0]`.
pub(crate) fn interpolate(value: f64, from: [f64; 2], to: [f64; 2]) -> f64 {
    if from[0] == from[1] {
        return to[0];
    }
    to[0] + ((value - from[0]) / (from[1] - from[0])) * (to[1] - to[0])
}

/// NaN becomes 0 and ±∞ become ±[`f64::MAX`]; finite values are unchanged.
pub(crate) fn make_finite(value: f64) -> f64 {
    if value.is_nan() {
        0.0
    } else {
        value.clamp(-f64::MAX, f64::MAX)
    }
}

/// Validates the interval list of a `Domain` or `Range`.
fn intervals(values: &[[f64; 2]]) -> Result<Box<[[f64; 2]]>> {
    finite(values.iter().flatten().copied())?;
    if values.iter().any(|r| r[0] > r[1]) {
        return Err(Error::syntax("reversed function range"));
    }
    Ok(values.into())
}

/// Validates the dimensions `m` and `n`, returning `n`.
fn dimensions(inputs: usize, outputs: usize) -> Result<usize> {
    if inputs == 0 || outputs == 0 {
        return Err(Error::syntax("wrong function input dimension"));
    }
    if inputs > MAX_COMPONENTS {
        return Err(Error::limit(
            "function input count exceeds implementation limit (Annex C)",
        ));
    }
    Ok(outputs)
}

/// Rejects parameters that no PDF number produces.
fn finite(mut values: impl Iterator<Item = f64>) -> Result<()> {
    if values.all(f64::is_finite) {
        Ok(())
    } else {
        Err(Error::syntax("function parameter is not finite"))
    }
}

#[cfg(test)]
pub(crate) mod tests;
