#![forbid(unsafe_code)]
#![warn(unreachable_pub)]
//! Bounded PDF stream filters (ISO 32000-2 7.4).
//!
//! This crate turns encoded stream bytes into decoded bytes. It knows the
//! filter vocabulary and parameters but nothing about documents: callers
//! resolve indirect references before building a [`FilterChain`], and the
//! security handler removes the `Crypt` filter before decoding. The image
//! codecs of Table 6 are named here ([`ImageCodec`]) and decoded by
//! `pagura-image`.
//!
//! Every decoder enforces a caller-supplied output cap *while* decoding, so
//! a small hostile input can never allocate more than the cap. Damaged data
//! that can be used anyway (for example a truncated Flate stream) is a
//! repair reported through [`Diagnostics`], never a silent success (ADR 0003).
//! The crate owns the `filter.` and `predictor.` tag areas: [`features`] and
//! [`repairs`].
//!
//! It also holds the inverse encoders built from the same pieces:
//! [`flate::compress`] (zlib) and [`png::encode`] (eight-bit Gray, RGB and
//! straight RGBA images). No product path uses them yet; ADR 0020 records
//! their consumers, the writer's Flate streams and the CLI's PNG output.

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

pub use filter::{CryptFilter, EarlyChange, Filter, FilterChain, ImageCodec};
pub use predictor::{Predictor, PredictorKind};

use pagura_core::{Diagnostics, Limits, Meter, Result};

/// Decodes explicitly unwrapped DEFLATE (RFC 1951 3.2), applying
/// the optional PDF predictor after inflation.
///
/// [`Filter::Flate`] accepts zlib and, with a reported repair, automatically
/// recognized raw input, preferring a valid zlib header. An unwrapped stored
/// block can coincidentally start with such a header; use this function when
/// the container identifies raw DEFLATE, avoiding that inherent format
/// ambiguity. PDF FlateDecode requires the zlib wrapper (7.4.4). This
/// explicit raw entry point does not report [`repairs::RAW_DEFLATE`].
///
/// `limits` bound the output and the work as for [`FilterChain::decode`].
///
/// # Errors
///
/// Returns the same bounded, positioned syntax/limit errors and reported
/// truncation/trailing-data repairs as [`Filter::decode`].
pub fn decode_raw_deflate(
    input: &[u8],
    limits: &Limits,
    predictor: Predictor,
    diagnostics: &mut Diagnostics,
) -> Result<Vec<u8>> {
    let mut work = Meter::new(limits.decode_work, bounded::DECODE_WORK);
    flate::raw(
        input,
        limits.decoded_bytes,
        predictor,
        &mut work,
        diagnostics,
    )
}
