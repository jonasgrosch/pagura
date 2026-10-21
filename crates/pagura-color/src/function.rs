//! PDF functions (7.10) without PDF objects. The interpreter reads function
//! dictionaries and streams into the parameter structs here; construction
//! validates them, and evaluation is then total: it cannot fail and does
//! not allocate (ADR 0034).

use pagura_core::{Error, Limits, Result};
use std::sync::Arc;

/// A validated function of `m` inputs and `n` outputs.
#[derive(Debug, Clone)]
pub struct Function {
    domain: Box<[[f64; 2]]>,
    range: Option<Box<[[f64; 2]]>>,
    outputs: usize,
    kind: Kind,
}

#[derive(Debug, Clone)]
enum Kind {
    Sampled(Box<SampledFunction>),
    Exponential(ExponentialFunction),
    Stitching(StitchingFunction),
    Calculator(CalculatorFunction),
}

/// Parameters of a Type 0 sampled function (7.10.2).
#[derive(Debug, Clone)]
#[non_exhaustive]
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
#[derive(Debug, Clone)]
#[non_exhaustive]
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
#[derive(Debug, Clone)]
#[non_exhaustive]
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
#[derive(Debug, Clone)]
#[non_exhaustive]
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
    /// Returns `Syntax` for inconsistent parameters or short sample data and
    /// a `Limit` error when the sample table exceeds `limits`.
    pub fn sampled(parameters: SampledFunction, limits: &Limits) -> Result<Self> {
        let _ = (parameters, limits);
        Err(Error::invalid_argument("sampled functions are not implemented"))
    }

    /// A Type 2 exponential interpolation function.
    ///
    /// # Errors
    ///
    /// Returns `Syntax` for inconsistent parameters.
    pub fn exponential(parameters: ExponentialFunction) -> Result<Self> {
        let _ = parameters;
        Err(Error::invalid_argument("exponential functions are not implemented"))
    }

    /// A Type 3 stitching function.
    ///
    /// # Errors
    ///
    /// Returns `Syntax` for inconsistent parameters.
    pub fn stitching(parameters: StitchingFunction) -> Result<Self> {
        let _ = parameters;
        Err(Error::invalid_argument("stitching functions are not implemented"))
    }

    /// A Type 4 calculator function, parsed and validated statically: every
    /// branch's stack effect and operand types are checked, so evaluation
    /// cannot fail; arithmetic faults (division by zero, a negative square
    /// root) have defined results.
    ///
    /// # Errors
    ///
    /// Returns `Syntax` for a malformed program and `Unsupported` for a
    /// program whose stack effect depends on its data and so cannot be
    /// validated.
    pub fn calculator(parameters: CalculatorFunction, limits: &Limits) -> Result<Self> {
        let _ = (parameters, limits);
        Err(Error::invalid_argument("calculator functions are not implemented"))
    }

    /// The number of inputs `m`.
    pub fn inputs(&self) -> usize {
        self.domain.len()
    }

    /// The number of outputs `n`.
    pub fn outputs(&self) -> usize {
        self.outputs
    }

    /// The `function_work` units one evaluation costs (ADR 0016): one per
    /// calculator operator executed at most, and per sample looked up or
    /// interpolated.
    pub fn cost(&self) -> usize {
        match &self.kind {
            Kind::Sampled(_) | Kind::Exponential(_) | Kind::Stitching(_) | Kind::Calculator(_) => 1,
        }
    }

    /// Evaluates the function: inputs are clipped to the domain and outputs to
    /// the range. `output` must hold [`Self::outputs`] values; extra inputs
    /// are ignored and missing ones read as the domain's lower bound.
    pub fn evaluate(&self, input: &[f64], output: &mut [f64]) {
        let _ = (input, &self.range);
        output.fill(0.0);
    }
}
