#![forbid(unsafe_code)]
//! Colour management without PDF objects: ICC.1:2010 forward transforms and
//! the CIE colorimetry that calibrated PDF spaces share (ADR 0028).
//!
//! [`Profile::parse`] reads an ICC profile from bytes; [`Profile::convert`]
//! maps source components to encoded sRGB. [`cie`] holds the XYZ, CIELAB and
//! sRGB conversions. Resolving PDF colour spaces stays in `pagura-render`.

pub mod cie;
pub mod features;
mod icc;

pub use icc::{Intent, Profile};
