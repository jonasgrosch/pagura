//! Construction of `pagura_color` functions from the parameters read here.
//!
//! The parameter records mirror `pagura_color`'s; constructing those needs
//! the record types to be buildable outside their crate, which the colour
//! package provides. Until then construction reports what the colour
//! crate's stub constructors report.

use pagura_color::Function;
use pagura_core::{Error, Limits, Result};
use std::sync::Arc;

/// Type 0 parameters (7.10.2).
#[derive(Debug, Clone)]
#[expect(dead_code, reason = "read once pagura-color's function records can be built here")]
pub(crate) struct SampledFunction {
    pub(crate) domain: Vec<[f64; 2]>,
    pub(crate) range: Vec<[f64; 2]>,
    pub(crate) size: Vec<u32>,
    pub(crate) bits_per_sample: u8,
    pub(crate) order: u8,
    pub(crate) encode: Option<Vec<[f64; 2]>>,
    pub(crate) decode: Option<Vec<[f64; 2]>>,
    pub(crate) samples: Arc<[u8]>,
}

/// Type 2 parameters (7.10.3).
#[derive(Debug, Clone)]
#[expect(dead_code, reason = "read once pagura-color's function records can be built here")]
pub(crate) struct ExponentialFunction {
    pub(crate) domain: [f64; 2],
    pub(crate) range: Option<Vec<[f64; 2]>>,
    pub(crate) c0: Vec<f64>,
    pub(crate) c1: Vec<f64>,
    pub(crate) exponent: f64,
}

/// Type 3 parameters (7.10.4).
#[derive(Debug, Clone)]
#[expect(dead_code, reason = "read once pagura-color's function records can be built here")]
pub(crate) struct StitchingFunction {
    pub(crate) domain: [f64; 2],
    pub(crate) range: Option<Vec<[f64; 2]>>,
    pub(crate) functions: Vec<Arc<Function>>,
    pub(crate) bounds: Vec<f64>,
    pub(crate) encode: Vec<[f64; 2]>,
}

/// Type 4 parameters (7.10.5).
#[derive(Debug, Clone)]
#[expect(dead_code, reason = "read once pagura-color's function records can be built here")]
pub(crate) struct CalculatorFunction {
    pub(crate) domain: Vec<[f64; 2]>,
    pub(crate) range: Vec<[f64; 2]>,
    pub(crate) program: Arc<[u8]>,
}

pub(crate) fn sampled(parameters: SampledFunction, limits: &Limits) -> Result<Function> {
    let _ = (parameters, limits);
    Err(Error::invalid_argument("sampled functions are not implemented"))
}

#[expect(clippy::needless_pass_by_value, reason = "the colour crate's constructor consumes it")]
pub(crate) fn exponential(parameters: ExponentialFunction) -> Result<Function> {
    let _ = parameters;
    Err(Error::invalid_argument("exponential functions are not implemented"))
}

#[expect(clippy::needless_pass_by_value, reason = "the colour crate's constructor consumes it")]
pub(crate) fn stitching(parameters: StitchingFunction) -> Result<Function> {
    let _ = parameters;
    Err(Error::invalid_argument("stitching functions are not implemented"))
}

pub(crate) fn calculator(parameters: CalculatorFunction, limits: &Limits) -> Result<Function> {
    let _ = (parameters, limits);
    Err(Error::invalid_argument("calculator functions are not implemented"))
}
