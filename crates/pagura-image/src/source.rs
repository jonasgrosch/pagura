//! Image sources decoded on request (ADR 0034): an image is read and
//! validated when content is interpreted, and its samples are decoded only
//! when painting asks for them, at the reduction and region it needs.

use crate::DecodedImage;
use pagura_core::{Diagnostic, Dictionary, Error, Limits, ObjectId, Recovery, Result};
use pagura_document::{Document, IndirectObject};

/// The identity of an image source within one document: never an address
/// that can be reused, so it can key decoded images in a `Store`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImageId {
    /// An image XObject.
    Object(ObjectId),
    /// An inline image: the content stream holding it (`None` for content
    /// that is not a stream, such as a generated appearance) and the offset
    /// of its `BI`.
    Inline {
        /// The content stream.
        stream: Option<ObjectId>,
        /// Offset of `BI` within that stream.
        offset: usize,
    },
}

/// An image whose dictionary has been read and validated and whose data is
/// held undecoded, apart from general-purpose filters. It does not refer to
/// the document, so it can be kept in a recorded scene.
///
/// It exposes what interpretation needs to describe the image without
/// decoding it: dimensions, components and precision, the colour space value
/// (or the JPEG 2000 container's colour), the decode array, interpolation and
/// the mask (colour key, stencil or soft-mask image, embedded alpha).
#[derive(Debug, Clone)]
pub struct ImageSource {
    id: ImageId,
    width: u32,
    height: u32,
}

/// How many samples a decode should produce.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Reduce {
    /// Every sample, exactly.
    #[default]
    Full,
    /// Exact block means where they are exact (a JPEG minified by 8 or more
    /// from DC terms of blocks that do not clamp) and full samples
    /// elsewhere: the result is the same as decoding fully and averaging.
    ExactMeans {
        /// The fewest samples per output pixel along x that painting needs.
        x: u32,
        /// The fewest samples per output pixel along y that painting needs.
        y: u32,
    },
    /// Reconstruction near the drawn size: JPEG cells (1, 2, 3, 4 or 6 per
    /// block axis) and JPEG 2000 resolution levels. Only under the
    /// `reduced_image_decode` fast parameter; not within the one-level
    /// contract.
    Drawn {
        /// Drawn width of the whole image in device pixels, rounded up.
        width: u32,
        /// Drawn height of the whole image in device pixels, rounded up.
        height: u32,
    },
}

/// A rectangle of source samples, `x0..x1` × `y0..y1`, top row first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SampleRect {
    /// First column.
    pub x0: u32,
    /// First row.
    pub y0: u32,
    /// One past the last column.
    pub x1: u32,
    /// One past the last row.
    pub y1: u32,
}

/// What painting asks a decode for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct DecodeRequest {
    /// The sample reduction.
    pub reduce: Reduce,
    /// The samples painting reads; rows and blocks outside may be skipped.
    /// `None` asks for the whole image.
    pub region: Option<SampleRect>,
}

/// The result of a decode: the samples and the repairs made while decoding,
/// which the caller reports in the image's object.
#[derive(Debug)]
#[non_exhaustive]
pub struct Decoded {
    /// The image.
    pub image: DecodedImage,
    /// Repairs made while decoding the data.
    pub diagnostics: Vec<Diagnostic>,
}

impl ImageSource {
    /// Reads and validates an image XObject (8.9.5) without decoding its
    /// samples.
    ///
    /// # Errors
    ///
    /// Returns `Syntax` for a malformed dictionary, `Unsupported` for
    /// unsupported codecs or features, and a `Limit` error for an image
    /// beyond `document.limits()`.
    pub fn from_xobject(document: &Document, image: &IndirectObject) -> Result<Self> {
        let _ = (document, image);
        Err(Error::invalid_argument("lazy image sources are not implemented"))
    }

    /// Reads and validates an inline image (8.9.7) given its dictionary
    /// (abbreviated keys allowed), its data and its identity.
    ///
    /// # Errors
    ///
    /// As [`Self::from_xobject`].
    pub fn from_inline(
        document: &Document,
        resources: &Dictionary,
        dictionary: &Dictionary,
        data: &[u8],
        id: ImageId,
    ) -> Result<Self> {
        let _ = (document, resources, dictionary, data, id);
        Err(Error::invalid_argument("lazy image sources are not implemented"))
    }

    /// The identity.
    pub fn id(&self) -> ImageId {
        self.id
    }

    /// Width in samples (`Width`).
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in samples (`Height`).
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Bytes this source retains.
    pub fn retained_bytes(&self) -> usize {
        size_of::<Self>()
    }

    /// An upper bound of the bytes and `decode_work` a decode of `request`
    /// uses, for reservation before decoding.
    pub fn decode_cost(&self, request: &DecodeRequest) -> (usize, usize) {
        let _ = request;
        (0, 0)
    }

    /// Decodes the samples `request` asks for.
    ///
    /// # Errors
    ///
    /// Returns `Syntax` for malformed data, `Unsupported` for unsupported
    /// codec features, and a `Limit` error when decoding exceeds `limits`.
    /// Under [`Recovery::Strict`] a repair is an error.
    pub fn decode(
        &self,
        request: &DecodeRequest,
        limits: &Limits,
        recovery: Recovery,
    ) -> Result<Decoded> {
        let _ = (request, limits, recovery);
        Err(Error::invalid_argument("lazy image sources are not implemented"))
    }
}
