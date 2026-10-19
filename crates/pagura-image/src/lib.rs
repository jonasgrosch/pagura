#![forbid(unsafe_code)]
#![warn(unreachable_pub)]
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

mod adapter;
mod dispatch;
mod filter_rules;
mod image;
mod inline;
mod mask;
mod parameters;
mod stream;
mod xobject;

pub use image::{DecodedImage, ImageAlpha, ImageKind, MaskKind, Precision, Samples};
pub use pagura_imagecodec::jpx::{EnumeratedColorSpace, JpxColor};

use pagura_core::{Dictionary, Limits, Result};
use pagura_document::{Document, IndirectObject};

/// Decodes a base image XObject stream with `/Subtype /Image` (Table 87).
/// Thumbnail dictionaries without that entry need a separate caller contract.
///
/// Errors and reported repairs carry the image's object; positions are
/// offsets in its decoded stream data, or the object's own position for
/// faults of the dictionary.
///
/// # Errors
///
/// Returns syntax errors for malformed image dictionaries or data,
/// `Unsupported` for codecs or features not implemented, and limit errors
/// when the image exceeds [`pagura_core::Limits`].
pub fn decode_xobject(document: &Document, image: &IndirectObject) -> Result<DecodedImage> {
    xobject::decode(document, image)
}

/// Decodes an inline image (`BI … ID … EI`, 8.9.7) given its dictionary
/// (abbreviated keys allowed) and its data bytes.
///
/// Positions in errors and reported repairs are offsets in `data`
/// ([`pagura_core::Position::Buffer`]), or unknown for faults of the
/// dictionary; the caller places them in its content stream.
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
}

/// The length of an inline image's data starting right after `ID` and its
/// single white-space byte, so the content lexer can resume after `EI`.
/// Uses `/L` (PDF 2.0) or the computed size for unfiltered data, and a
/// bounded scan for `EI` otherwise, whose candidate boundaries are checked
/// by decoding under `limits`.
///
/// # Errors
///
/// Returns a syntax error if the end cannot be determined, or a limit error
/// when checking a candidate boundary exceeds `limits`.
pub fn inline_data_length(
    dictionary: &Dictionary,
    content: &[u8],
    limits: &Limits,
) -> Result<usize> {
    inline::data_length(dictionary, content, limits)
}

#[cfg(test)]
mod tests;
