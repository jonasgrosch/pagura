//! Baseline, extended and progressive JPEG decoding (ITU-T T.81) for
//! `DCTDecode` (clause 7.4.8).

use pagura_core::{Error, Feature, Limits, Result};

/// Placeholder tag while the decoder is under construction.
const PENDING: Feature = Feature::new("filter.DCTDecode");

/// A decoded JPEG image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DctImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Components per pixel (1, 3 or 4).
    pub components: u8,
    /// Interleaved 8-bit samples after any colour transform, top row first.
    pub samples: Vec<u8>,
}

/// Decodes a JPEG stream. `color_transform` is `/ColorTransform` from the
/// filter's parameters (clause 7.4.8, Table 13), if present.
///
/// # Errors
///
/// Returns syntax errors for corrupt data and limit errors beyond `limits`.
pub fn decode(data: &[u8], color_transform: Option<i64>, limits: &Limits) -> Result<DctImage> {
    let _ = (data, color_transform, limits);
    Err(Error::unsupported(
        PENDING,
        "DCT decoding is not implemented yet",
    ))
}
