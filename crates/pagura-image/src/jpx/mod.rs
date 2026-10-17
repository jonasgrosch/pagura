//! JPEG 2000 decoding (ISO/IEC 15444-1) for `JPXDecode` (clause 7.4.9):
//! JP2 files and raw codestreams.
//!
//! Supports 1–16-bit components, multiple tiles and packet-boundary tile-parts,
//! default and explicit precincts, all five progression orders and POC changes,
//! PPM/PPT packed headers, complete code-block bit planes across quality layers,
//! ROI maxshift, MQ and raw bypass coding, context reset, pass termination,
//! predictable termination, vertical-causal contexts and segmentation symbols,
//! 5/3 and 9/7 synthesis, RCT/ICT, palettes and straight opacity.
//! Format mechanics remain pending primary T.800/T.801 review; independently
//! encoded source-sample vectors are compared with OpenJPEG. This is separate
//! from the PDF rules verified against clause 7.4.9 and Table 87.
//! Unsupported JPX extensions and sample representations return
//! `Unsupported(filter.JPXDecode)`; no partial image is returned.
//! ICC bytes and enumerated colour identifiers are preserved for the caller.
//!
//! `Limits::stream_bytes` caps encoded input, `pixels` caps the reference-grid
//! image, `decoded_bytes` conservatively caps cumulative allocations (including
//! scratch buffers), `objects` caps tiles, and `operations` caps parsing,
//! tag-tree and coefficient-pass work. All allocations are charged first.

mod boxes;
mod codestream;
mod coding;
mod io;
mod mq;
mod packed;
mod packet;
mod progression;
mod segments;
mod tier1;
mod wavelet;

use pagura_core::{Error, Feature, Limits, Position, Result};

/// Stable tag for JPEG 2000 modes not yet implemented.
const UNSUPPORTED: Feature = Feature::new("filter.JPXDecode");

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
    if data.len() > limits.stream_bytes {
        return Err(Error::limit("JPEG 2000 input budget exceeded").at(Position::Buffer(0)));
    }
    let mut budget = io::Budget::new(limits);
    decode_bounded(data, &mut budget).map_err(|error| error.at(Position::Buffer(0)))
}

fn decode_bounded(data: &[u8], budget: &mut io::Budget<'_>) -> Result<JpxImage> {
    if data.starts_with(&[255, 79]) {
        codestream::decode(data, 0, budget)
    } else {
        let (stream, base, header) = boxes::parse(data, budget)?;
        let mut image = codestream::decode(stream, base, budget)?;
        header.apply(&mut image, budget)?;
        Ok(image)
    }
}

fn unsupported(message: &'static str, offset: usize) -> Error {
    Error::unsupported(UNSUPPORTED, message).at(Position::Buffer(offset))
}

#[cfg(test)]
mod tests;
