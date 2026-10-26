#![forbid(unsafe_code)]
//! Colour management without PDF objects (ADR 0028, ADR 0034): resolved PDF
//! colour spaces, functions, prepared conversions into blending spaces,
//! ICC.1:2010 forward transforms and CIE colorimetry.
//!
//! The interpreter reads colour-space and function objects and builds
//! [`ColorSpace`] and [`Function`] values; rendering prepares a
//! [`Converter`] per colour space and target once, and evaluates functions
//! and conversions without allocating or failing.

mod affine;
mod batch;
pub mod cie;
mod components;
mod convert;
pub mod features;
mod function;
mod icc;
mod ramp;
mod space;

pub use components::Components;
pub use convert::{
    Colorants, Conversion, Converter, Encoder, MAX_CHANNELS, Painted, Process, Target, luminosity,
};
pub use function::{
    CalculatorFunction, ExponentialFunction, Function, SampledFunction, StitchingFunction,
};
pub use icc::{Intent, Profile};
pub use ramp::{APPROXIMATE, MAX_RAMP_CHANNELS, Piece, Ramp, RampStage};
pub use space::{ColorSpace, Colorant, DeviceN, IccSpace, Indexed, MAX_COMPONENTS, Separation};
