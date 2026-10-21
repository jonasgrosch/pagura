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
pub mod deferred;
pub mod features;
pub mod jbig2;
pub mod jpx;

use pagura_core::{Dictionary, Error, ObjectId, Result, Value};
use pagura_document::{Document, IndirectObject};

/// Stable reported repairs of malformed image dictionaries and data (ADR 0003).
pub mod tags {
    use pagura_core::Repair;
    /// Missing packed bytes become zero, including a missing low byte of a
    /// 16-bit sample; dimensions and row padding remain authoritative (8.9.3,
    /// 8.9.5.1). The fill value is a reader policy, not specified by ISO.
    pub const DATA_SHORT: Repair = Repair::new("image.data-short");
    /// Bytes beyond the declared sample rows are ignored (8.9.3, 8.9.5.1).
    pub const DATA_EXCESS: Repair = Repair::new("image.data-excess");
    /// Ignore a stencil's extraneous colour space (8.9.5.1, Table 87).
    pub const STENCIL_COLOR_SPACE: Repair = Repair::new("image.stencil-color-space");
    /// Recognize Table 92 filter abbreviations on an XObject (8.9.7).
    pub const FILTER_ABBREVIATION: Repair = Repair::new("image.filter-abbreviation");
    /// Treat a serialized `(null)` parameter as absent, or ignore a
    /// wrong-typed parameter for parameterless JPXDecode (7.3.9, 7.3.8.1
    /// Table 5, 7.4.1, 7.4.9). Actual null values require no repair.
    pub const DECODE_PARAMS: Repair = Repair::new("image.decode-params");
    /// Decode fax rows using Columns, then crop or white-pad to the image
    /// Width (7.4.6 Table 11; 8.9.5.1 Table 87). Row padding is not a sample.
    pub const CCITT_COLUMNS: Repair = Repair::new("image.ccitt-columns");
    /// Decode at the image's sample precision after a syntax failure with
    /// conflicting valid predictor precision, only if the retry succeeds
    /// (7.4.4.3 Table 10).
    pub const PREDICTOR_PRECISION: Repair = Repair::new("image.predictor-precision");
    /// T.800 A.4.2: ordered tile-parts exceeding their declared count use
    /// their actual, length-delimited sequence through EOC.
    pub const JPX_TILE_PART_COUNT: Repair = Repair::new("jpx.tile-part-count");
    /// T.800 I.5.3.4–I.5.3.5: a sole top-level cmap before the codestream
    /// supplies the header palette's missing component mapping.
    pub const JPX_MAPPING_LOCATION: Repair = Repair::new("jpx.mapping-box-location");
    /// 8.9.5 Table 87: Subtype Image identifies an image XObject despite a
    /// malformed optional Type entry. Missing/invalid Subtype remains an error.
    pub const XOBJECT_TYPE: Repair = Repair::new("image.xobject-type");
    /// 7.4.4, 7.4.8: DCTDecode data with predictor parameters is decoded as
    /// Flate only after strict framing/checksum/predictor validation and an
    /// exact match to the declared eight-bit image sample count.
    pub const FILTER_MISMATCH: Repair = Repair::new("image.filter-mismatch");
}

mod dispatch;
mod fax_image;
mod filter_rules;
mod inline;
mod jpx_channels;
mod jpx_image;
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
        color: jpx::JpxColor,
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
    /// A baseline YCbCr JPEG in its component planes, from
    /// [`deferred::SampleLayout::Deferred`]. Values are RGB bytes on demand.
    Planar(Box<deferred::Planar>),
    /// One-bit, one-component samples in packed rows, from
    /// [`deferred::SampleLayout::Deferred`].
    Packed(deferred::Packed),
}

impl Samples {
    /// Number of values.
    pub fn len(&self) -> usize {
        match self {
            Self::Bytes(values) => values.len(),
            Self::Words(values) | Self::Mixed { values, .. } => values.len(),
            Self::Planar(planar) => planar.width() * planar.height() * 3,
            Self::Packed(packed) => packed.width() * packed.height(),
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
            Self::Planar(planar) => planar.get(index),
            Self::Packed(packed) => packed.get(index),
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
    /// Present when a JPEG image was reconstructed near its drawn size;
    /// `width` and `height` then count its reduced cells.
    pub reduction: Option<Reduction>,
}

/// A JPEG image reconstructed with fewer samples than its dictionary
/// declares, in the [`deferred::SampleLayout::Cells`] layout: an
/// approximate path (ADR 0004).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reduction {
    /// Samples kept per eight-sample block axis: 1, 2, 3, 4 or 6.
    pub cells: u8,
    /// The declared `/Width` in samples (Table 87).
    pub width: u32,
    /// The declared `/Height` in samples (Table 87).
    pub height: u32,
}

impl Reduction {
    /// The declared image extent in reduced cells. A final cell that
    /// reaches into its block's padding covers only part of a cell.
    pub fn extent(&self) -> (f64, f64) {
        let scale = f64::from(self.cells) / 8.0;
        (
            f64::from(self.width) * scale,
            f64::from(self.height) * scale,
        )
    }
}

impl DecodedImage {
    /// The extent of the image's unit square in stored samples: its
    /// dimensions, or the declared extent of a [`Reduction`].
    pub fn extent(&self) -> (f64, f64) {
        self.reduction.map_or(
            (f64::from(self.width), f64::from(self.height)),
            |reduction| reduction.extent(),
        )
    }

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
    decode_xobject_with(document, image, deferred::SampleLayout::Interleaved, None)
}

/// Decodes a base image XObject as [`decode_xobject`], keeping samples in
/// `layout` where it applies, in storage from `buffers` when given. Errors
/// and diagnostics are those of [`decode_xobject`]: a deferred decode that
/// fails is repeated in the interleaved layout.
///
/// # Errors
///
/// As [`decode_xobject`].
pub fn decode_xobject_with(
    document: &Document,
    image: &IndirectObject,
    layout: deferred::SampleLayout,
    buffers: Option<&deferred::SampleBuffers>,
) -> Result<DecodedImage> {
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
                    tags::XOBJECT_TYPE,
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
        dispatch::decode_with(
            document,
            &dictionary,
            stream.data,
            stream.codec.as_ref(),
            (layout, buffers),
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
        pagura_core::Position::Unknown => image.source().position(),
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
