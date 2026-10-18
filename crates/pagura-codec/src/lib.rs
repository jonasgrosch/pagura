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
mod encoding;
mod filter;
pub mod flate;
mod lzw;
mod output;
pub mod png;
mod predictor;
mod report;

pub use filter::{Filter, FilterChain, ImageCodec};
pub use predictor::{Predictor, PredictorKind};

use pagura_core::{Diagnostics, Result};

/// Stable tags for filters this crate does not decode.
pub mod features {
    use pagura_core::Feature;

    /// Predictor dimensions exceed the stable API representation.
    pub const PREDICTOR_DIMENSIONS: Feature = Feature::new("predictor.dimensions");

    /// `DCTDecode` (JPEG); decoded by the image layer, not implemented yet.
    pub const DCT: Feature = Feature::new("filter.DCTDecode");
    /// `JPXDecode` (JPEG 2000); not implemented yet.
    pub const JPX: Feature = Feature::new("filter.JPXDecode");
    /// `JBIG2Decode`; not implemented yet.
    pub const JBIG2: Feature = Feature::new("filter.JBIG2Decode");
    /// `CCITTFaxDecode`; not implemented yet.
    pub const CCITT_FAX: Feature = Feature::new("filter.CCITTFaxDecode");
}

/// Stable, security-relevant repairs for damaged filter input (ADR 0003).
pub mod tags {
    use pagura_core::Repair;
    /// FlateDecode data lacks the zlib wrapper; decoded as raw DEFLATE.
    /// ISO 32000-2 clause 7.4.4 requires RFC 1950 framing; Strict rejects
    /// this repair. Explicit `decode_raw_deflate` does not apply it.
    pub const RAW_DEFLATE: Repair = Repair::new("filter.raw-deflate");
    /// Keep the decoded prefix when input ends before the required end marker.
    /// Incomplete predictor rows remain errors; no synthetic pixels are added.
    pub const TRUNCATED: Repair = Repair::new("filter.truncated");
    /// Keep a full 12-bit LZW dictionary through entry 4095 without further
    /// insertions until clear or EOD; report missing clear once per stream.
    /// This recovers an encoder fault under ISO 32000-2 clause 7.4.4.2.
    pub const LZW_FULL_TABLE: Repair = Repair::new("filter.lzw-full-table");
    /// Ignore nonzero unused bits after LZW EOD. ISO 32000-2 clause 7.4.4.2
    /// requires the encoder to zero them; Strict rejects this repair.
    pub const LZW_PADDING: Repair = Repair::new("filter.lzw-padding");
    /// Start with the initial literal dictionary when the leading clear code
    /// is absent and the first code is a literal or EOD (7.4.4.2).
    pub const LZW_INITIAL_CLEAR: Repair = Repair::new("filter.lzw-initial-clear");
    /// Keep decoded bytes despite a mismatched zlib Adler-32 checksum.
    pub const CHECKSUM_MISMATCH: Repair = Repair::new("filter.checksum-mismatch");
    /// Ignore bytes after the first end marker. ASCII filters permit trailing
    /// PDF whitespace; binary filters report any trailing byte, including zero.
    pub const TRAILING_DATA: Repair = Repair::new("filter.trailing-data");
}

/// Decodes explicitly unwrapped DEFLATE (RFC 1951 section 3.2), applying
/// the optional PDF predictor after inflation.
///
/// [`Filter::Flate`] accepts zlib and, with a reported repair, automatically
/// recognized raw input, preferring a valid zlib header. An unwrapped stored block can coincidentally
/// start with such a header; use this function when the container identifies
/// raw DEFLATE, avoiding that inherent format ambiguity. PDF FlateDecode
/// requires the zlib wrapper (ISO 32000-2 clause 7.4.4). This explicit raw
/// entry point does not report [`tags::RAW_DEFLATE`].
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
