#![forbid(unsafe_code)]
//! Bounded PDF stream filters (ISO 32000-2 clause 7.4).
//!
//! This crate turns encoded stream bytes into decoded bytes. It knows the
//! filter vocabulary and parameters but nothing about documents: callers
//! resolve indirect references before building a [`FilterChain`], and the
//! security handler removes the `Crypt` filter before decoding.
//! [`flate::compress`] provides zlib output for stream writing; [`png::encode`]
//! encodes eight-bit Gray, RGB and straight RGBA images without host capabilities.
//!
//! Every decoder enforces a caller-supplied output cap *while* decoding, so
//! a small hostile input can never allocate more than the cap. Damaged data
//! that can be used anyway (for example a truncated Flate stream) is a
//! repair reported through [`Diagnostics`], never a silent success (ADR 0003).

mod ascii;
mod bounded;
pub mod features;
mod filter;
pub mod flate;
mod lzw;
mod output;
pub mod png;
mod predictor;
pub mod repairs;
mod report;
mod run_length;

pub use filter::{EarlyChange, Filter, FilterChain, ImageCodec};
pub use predictor::{Predictor, PredictorKind};

use pagura_core::{Diagnostics, Result};

/// Decodes explicitly unwrapped DEFLATE (RFC 1951 section 3.2), applying
/// the optional PDF predictor after inflation.
///
/// [`Filter::Flate`] accepts zlib and, with a reported repair, automatically
/// recognized raw input, preferring a valid zlib header. An unwrapped stored block can coincidentally
/// start with such a header; use this function when the container identifies
/// raw DEFLATE, avoiding that inherent format ambiguity. PDF FlateDecode
/// requires the zlib wrapper (ISO 32000-2 clause 7.4.4). This explicit raw
/// entry point does not report [`repairs::RAW_DEFLATE`].
///
/// # Errors
///
/// Returns the same bounded, positioned syntax/limit errors and reported
/// truncation/trailing-data repairs as [`Filter::decode`].
pub fn decode_raw_deflate(
    input: &[u8],
    max_output: usize,
    predictor: Predictor,
    diagnostics: &mut Diagnostics,
) -> Result<Vec<u8>> {
    flate::raw(input, max_output, predictor, diagnostics)
}
