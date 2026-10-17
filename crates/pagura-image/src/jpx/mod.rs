//! JPEG 2000 decoding (ISO/IEC 15444-1) for `JPXDecode` (clause 7.4.9):
//! JP2 files and raw codestreams.

use pagura_core::{Error, Feature, Limits, Result};

/// Placeholder tag while the decoder is under construction.
const PENDING: Feature = Feature::new("filter.JPXDecode");

/// Colour information carried by a JP2 file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JpxColor {
    /// No colour box (raw codestream).
    Unspecified,
    /// An enumerated colour space (`colr` method 1), e.g. 16 = sRGB, 17 =
    /// greyscale, 18 = sYCC.
    Enumerated(u32),
    /// A restricted or full ICC profile (`colr` methods 2 and 3).
    Icc(Vec<u8>),
}

/// One decoded component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JpxComponent {
    /// Bit depth of the samples.
    pub bits: u8,
    /// Whether samples are signed.
    pub signed: bool,
    /// Width in samples (components may be subsampled).
    pub width: u32,
    /// Height in samples.
    pub height: u32,
    /// Samples, row-major, offset to unsigned when `signed`.
    pub samples: Vec<u16>,
}

/// A decoded JPEG 2000 image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JpxImage {
    /// Image width on the reference grid.
    pub width: u32,
    /// Image height on the reference grid.
    pub height: u32,
    /// Components after any palette and channel definitions.
    pub components: Vec<JpxComponent>,
    /// Colour information.
    pub color: JpxColor,
    /// Index of an opacity component (`cdef`), if any.
    pub alpha: Option<usize>,
}

/// Decodes a JP2 file or raw JPEG 2000 codestream.
///
/// # Errors
///
/// Returns syntax errors for corrupt data, `Unsupported` for features not
/// implemented, and limit errors beyond `limits`.
pub fn decode(data: &[u8], limits: &Limits) -> Result<JpxImage> {
    let _ = (data, limits);
    Err(Error::unsupported(
        PENDING,
        "JPEG 2000 decoding is not implemented yet",
    ))
}
