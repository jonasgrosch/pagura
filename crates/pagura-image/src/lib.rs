#![forbid(unsafe_code)]
//! PDF images (ISO 32000-2 clause 8.9; ADR 0010 and ADR 0027): image
//! XObjects and inline images decoded to samples. The DCT (JPEG), CCITT fax,
//! JBIG2 and JPEG 2000 codecs live in `pagura-imagecodec`; this crate reads
//! image dictionaries, adapts them to the codecs and places codec positions
//! in the document.
//!
//! Decoding produces samples in the image's own color space plus everything
//! needed to paint it (decode array, masks, interpolation). Color conversion
//! and painting belong to `pagura-render`.

pub mod features;
pub mod repairs;

use pagura_core::{Dictionary, Error, ObjectId, Result, Value};
use pagura_document::{Document, IndirectObject};
pub use pagura_imagecodec::jpx::JpxColor;

mod adapter;
mod dispatch;
mod filter_rules;
mod inline;
mod mask;
mod parameters;
mod stream;

/// How an image's samples are interpreted.
#[derive(Debug, Clone, PartialEq)]
pub enum ImageKind {
    /// Colour samples in a resolved PDF `color_space`. Successful decoding
    /// supplies a colour space here or uses [`Self::EmbeddedColor`].
    Color {
        /// The `/ColorSpace` value, resolved.
        color_space: Option<Value>,
    },
    /// Colour interpretation supplied by the JPEG 2000 container rather than
    /// a PDF colour-space value (7.4.9). Profiles and enumerated spaces are
    /// retained so painting can apply their original interpretation.
    EmbeddedColor {
        /// Colour description from the container.
        color: JpxColor,
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
    /// Legacy marker for soft-mask data embedded in a JPEG 2000 stream.
    /// Decoding now returns [`Self::Embedded`] with the actual opacity samples.
    InData,
    /// A decoded opacity channel, separate from ordinary colour samples.
    Embedded(ImageAlpha),
}

/// A JPX opacity plane (7.4.9 and Table 87). Its dimensions equal the base
/// image's dimensions. Samples are raw integers; normalize by their precision
/// at painting time. The alpha-is-shape graphics state remains a paint concern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageAlpha {
    /// Opacity precision, independent of colour component precision.
    pub bits_per_component: u8,
    /// One opacity sample per image pixel, in source row order.
    pub samples: Samples,
    /// Whether colour samples are already multiplied by this opacity
    /// (`SMaskInData=2`); unpremultiplication is deferred to painting.
    pub premultiplied: bool,
}

/// Unpacked samples, one value per component. Images of up to 8 bits per
/// component use one byte per value; 16-bit images use two. Mixed-precision
/// JPX samples use unsigned 16-bit values plus per-component precisions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Samples {
    /// Ordinary 1/2/4/8-bit values, or JPX values of 1–8 bits, in one byte.
    Bytes(Vec<u8>),
    /// Ordinary 16-bit values, or JPX values of 9–16 bits.
    Words(Vec<u16>),
    /// Interleaved JPX values whose colour components have different
    /// precisions (7.4.9, 8.9.5.2). No precision conversion is applied.
    Mixed {
        /// One unsigned value per component per pixel.
        values: Vec<u16>,
        /// Precision of each ordinary colour component, in interleave order.
        bits: Vec<u8>,
    },
}

impl Samples {
    /// Number of values.
    pub fn len(&self) -> usize {
        match self {
            Self::Bytes(values) => values.len(),
            Self::Words(values) | Self::Mixed { values, .. } => values.len(),
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
            Self::Words(values) | Self::Mixed { values, .. } => values.get(index).copied(),
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
    /// Bits per component (1, 2, 4, 8 or 16 for ordinary PDF samples;
    /// 1–16 for JPX). Zero denotes mixed JPX precision; use
    /// [`Self::component_bits`] for the precision of an individual component.
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

impl DecodedImage {
    /// Device fallback for an embedded colour description that the painter
    /// cannot support (7.4.9). The original profile or enumerated space remains
    /// available in [`ImageKind::EmbeddedColor`].
    pub fn embedded_color_fallback(&self) -> Option<Value> {
        if !matches!(self.kind, ImageKind::EmbeddedColor { .. }) {
            return None;
        }
        match self.components {
            1 => Some(Value::name(b"DeviceGray")),
            3 => Some(Value::name(b"DeviceRGB")),
            4 => Some(Value::name(b"DeviceCMYK")),
            _ => None,
        }
    }

    /// Precision of the ordinary colour component at `index`, or None when
    /// that component does not exist. Alpha has its own precision.
    pub fn component_bits(&self, index: usize) -> Option<u8> {
        if index >= usize::from(self.components) {
            return None;
        }
        match &self.samples {
            Samples::Mixed { bits, .. } => bits.get(index).copied(),
            _ => Some(self.bits_per_component),
        }
    }
}

/// Decodes a base image XObject stream with `/Subtype /Image` (Table 87).
/// Thumbnail dictionaries without that entry need a separate caller contract.
///
/// # Errors
///
/// Returns syntax errors for malformed image dictionaries or data,
/// `Unsupported` for codecs or features not implemented, and limit errors
/// when the image exceeds [`pagura_core::Limits`] (pixels, decoded bytes).
pub fn decode_xobject(document: &Document, image: &IndirectObject) -> Result<DecodedImage> {
    let mut diagnostics = pagura_core::Diagnostics::new(document.recovery());
    let result = (|| {
        let dictionary = image
            .dictionary()
            .ok_or(Error::syntax("image is not a dictionary"))?;
        if dictionary
            .get(b"Subtype")
            .map(|v| document.resolve(v))
            .transpose()?
            .and_then(Value::as_name)
            .is_none_or(|n| n != b"Image")
        {
            return Err(Error::syntax("image XObject requires Subtype Image"));
        }
        if let Some(kind) = dictionary
            .get(b"Type")
            .map(|v| document.resolve(v))
            .transpose()?
            && !kind.is_null()
            && kind.as_name().is_none_or(|n| n != b"XObject")
        {
            diagnostics.repair(
                pagura_core::Diagnostic::new(
                    repairs::XOBJECT_TYPE,
                    "Subtype Image identifies XObject; invalid Type ignored (8.9.5 Table 87)",
                )
                .at(pagura_core::Position::Buffer(0)),
            )?;
        }
        let dictionary = parameters::resolved_dictionary(document, dictionary, &mut diagnostics)?;
        filter_rules::validate(document, &dictionary, false, &mut diagnostics)?;
        let (width, height) = parameters::dimensions(&dictionary)?;
        parameters::budget(width, height, 1, 1, &document.limits())?;
        let stream = stream::read(document, image, &mut diagnostics)?;
        dispatch::decode(
            document,
            &dictionary,
            &stream.data,
            stream.codec.as_ref(),
            &mut diagnostics,
        )
    })();
    document.report(diagnostics.take().into_iter().map(|mut diagnostic| {
        diagnostic.position = image_position(diagnostic.position, image);
        diagnostic.in_object(image.id())
    }));
    result.map_err(|error: Error| {
        let position = image_position(error.position, image);
        let mut error = error.in_object(image.id());
        error.position = position;
        error
    })
}

fn image_position(
    position: pagura_core::Position,
    image: &IndirectObject,
) -> pagura_core::Position {
    match position {
        pagura_core::Position::Buffer(offset) => pagura_core::Position::Stream {
            object: image.id(),
            offset,
        },
        pagura_core::Position::Unknown => match image.source() {
            pagura_document::ObjectSource::File(range) => pagura_core::Position::File(range.start),
            pagura_document::ObjectSource::Compressed { .. } => pagura_core::Position::Unknown,
        },
        position => position,
    }
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

#[cfg(test)]
mod pdf_tests;
