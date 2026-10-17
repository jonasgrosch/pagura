#![forbid(unsafe_code)]
//! PDF images (ISO 32000-2 clause 8.9) and image codecs (clauses 7.4.6–7.4.10;
//! ADR 0010): image XObjects and inline images decoded to samples, with the
//! DCT (JPEG), CCITT fax, JBIG2 and JPEG 2000 codecs.
//!
//! Decoding produces samples in the image's own colour space plus everything
//! needed to paint it (decode array, masks, interpolation). Colour conversion
//! and painting belong to `pagura-render`.

pub mod ccitt;
pub mod dct;
pub mod features;
pub mod jbig2;
pub mod jpx;

use pagura_core::{Dictionary, Error, ObjectId, Result, Value};
use pagura_document::{Document, IndirectObject};

mod dispatch;
mod inline;
mod mask;
mod parameters;

/// How an image's samples are interpreted.
#[derive(Debug, Clone, PartialEq)]
pub enum ImageKind {
    /// Colour samples in `color_space` (resolved; `None` only for JPEG 2000
    /// images that carry their own colour information).
    Color {
        /// The `/ColorSpace` value, resolved.
        color_space: Option<Value>,
    },
    /// A 1-bit stencil mask painted with the current fill colour
    /// (`/ImageMask true`); `inverted` follows `/Decode [1 0]`.
    Stencil {
        /// Whether sample 1 (rather than 0) paints.
        inverted: bool,
    },
}

/// The masking of a colour image (clause 8.9.6).
#[derive(Debug, Clone, PartialEq)]
pub enum MaskKind {
    /// No mask.
    None,
    /// Colour-key masking: inclusive sample ranges per component.
    ColorKey(Vec<(u16, u16)>),
    /// An explicit stencil-mask image XObject.
    Stencil(ObjectId),
    /// A soft-mask image XObject (`/SMask`).
    Soft(ObjectId),
    /// Soft-mask data embedded in a JPEG 2000 stream (`/SMaskInData`).
    InData,
}

/// Unpacked samples, one value per component. Images of up to 8 bits per
/// component use one byte per value; 16-bit images use two.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Samples {
    /// Values of 1, 2, 4 or 8 bits, each in one byte.
    Bytes(Vec<u8>),
    /// 16-bit values.
    Words(Vec<u16>),
}

impl Samples {
    /// Number of values.
    pub fn len(&self) -> usize {
        match self {
            Self::Bytes(values) => values.len(),
            Self::Words(values) => values.len(),
        }
    }

    /// Whether there are no values.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The value at `index`, widened to `u16`.
    pub fn get(&self, index: usize) -> Option<u16> {
        match self {
            Self::Bytes(values) => values.get(index).copied().map(u16::from),
            Self::Words(values) => values.get(index).copied(),
        }
    }
}

/// A decoded image: unpacked samples plus painting parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedImage {
    /// Width in samples.
    pub width: u32,
    /// Height in samples.
    pub height: u32,
    /// Bits per component of the samples (1, 2, 4, 8 or 16).
    pub bits_per_component: u8,
    /// Components per sample.
    pub components: u8,
    /// Row-major samples, top row first, components interleaved.
    pub samples: Samples,
    /// Sample interpretation.
    pub kind: ImageKind,
    /// The `/Decode` array, if present.
    pub decode: Option<Vec<f64>>,
    /// Masking.
    pub mask: MaskKind,
    /// `/Interpolate`.
    pub interpolate: bool,
}

/// Decodes an image XObject stream.
///
/// # Errors
///
/// Returns syntax errors for malformed image dictionaries or data,
/// `Unsupported` for codecs or features not implemented, and limit errors
/// when the image exceeds [`pagura_core::Limits`] (pixels, decoded bytes).
pub fn decode_xobject(document: &Document, image: &IndirectObject) -> Result<DecodedImage> {
    (|| {
        let dictionary = image
            .dictionary()
            .ok_or(Error::syntax("image is not a dictionary"))?;
        for (key, expected) in [
            (b"Type".as_slice(), b"XObject".as_slice()),
            (b"Subtype", b"Image"),
        ] {
            if let Some(value) = dictionary.get(key).filter(|v| !v.is_null())
                && document
                    .resolve(value)?
                    .as_name()
                    .is_none_or(|n| n != expected)
            {
                return Err(Error::syntax("invalid image XObject type"));
            }
        }
        let dictionary = parameters::resolved_dictionary(document, dictionary)?;
        let (width, height) = parameters::dimensions(&dictionary)?;
        parameters::budget(width, height, 1, 1, &document.limits())?;
        let stream = document.image_stream(image)?;
        dispatch::decode(document, &dictionary, &stream.data, stream.codec.as_ref())
    })()
    .map_err(|error: Error| {
        let position = match error.position {
            pagura_core::Position::Buffer(offset) => pagura_core::Position::Stream {
                object: image.id(),
                offset,
            },
            pagura_core::Position::Unknown => match image.source() {
                pagura_document::ObjectSource::File(range) => {
                    pagura_core::Position::File(range.start)
                }
                pagura_document::ObjectSource::Compressed { .. } => pagura_core::Position::Unknown,
            },
            position => position,
        };
        let mut error = error.in_object(image.id());
        error.position = position;
        error
    })
}

/// Decodes an inline image (`BI … ID … EI`, clause 8.9.7) given its
/// dictionary (abbreviated keys allowed) and its data bytes.
///
/// # Errors
///
/// As [`decode_xobject`].
pub fn decode_inline(
    document: &Document,
    resources: &Dictionary,
    dictionary: &Dictionary,
    data: &[u8],
) -> Result<DecodedImage> {
    inline::decode(document, resources, dictionary, data)
        .map_err(|error| error.at(pagura_core::Position::Buffer(0)))
}

/// The length of an inline image's data starting right after `ID` and its
/// single white-space byte, so the content lexer can resume after `EI`.
/// Uses `/L` (PDF 2.0) or the computed size for unfiltered data, and a
/// bounded scan for `EI` otherwise.
///
/// # Errors
///
/// Returns a syntax error if the end cannot be determined.
pub fn inline_data_length(dictionary: &Dictionary, content: &[u8]) -> Result<usize> {
    inline::data_length(dictionary, content)
        .map_err(|error| error.at(pagura_core::Position::Buffer(0)))
}

#[cfg(test)]
mod tests;
