//! JBIG2 decoding (ITU-T T.88) for `JBIG2Decode` (clause 7.4.7): the
//! embedded stream organization with optional global segments.

use pagura_core::{Error, Feature, Limits, Result};

/// Placeholder tag while the decoder is under construction.
const PENDING: Feature = Feature::new("filter.JBIG2Decode");

/// A decoded page bitmap: 1-bit rows, MSB first, padded to bytes, 1 = black.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bitmap {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Packed rows.
    pub data: Vec<u8>,
}

/// Decodes a JBIG2 page from the stream's segments plus the
/// `/JBIG2Globals` stream data, if any.
///
/// # Errors
///
/// Returns syntax errors for corrupt data, `Unsupported` for segment types
/// not implemented, and limit errors beyond `limits`.
pub fn decode(data: &[u8], globals: Option<&[u8]>, limits: &Limits) -> Result<Bitmap> {
    let _ = (data, globals, limits);
    Err(Error::unsupported(
        PENDING,
        "JBIG2 decoding is not implemented yet",
    ))
}
