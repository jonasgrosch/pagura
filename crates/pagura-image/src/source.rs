//! Image sources decoded on request (ADR 0034): an image is read and
//! validated when content is interpreted, and its samples are decoded only
//! when painting asks for them, at the reduction and region it needs.

mod codec;
mod header;
#[cfg(test)]
mod tests;

use crate::deferred::SampleLayout;
use crate::{DecodedImage, ImageKind};
use header::{Codec, Header};
use pagura_core::{
    Diagnostic, Diagnostics, Dictionary, Error, Limits, ObjectId, Position, Recovery, Result, Value,
};
use pagura_document::{Document, IndirectObject};
use std::sync::Arc;

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

/// The masking of an image (8.9.6) as its dictionary declares it.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum SourceMask {
    /// No mask.
    None,
    /// Colour-key masking: inclusive ranges of raw sample values, one per
    /// colour component, validated against the components' precisions.
    ColorKey(Vec<[u16; 2]>),
    /// An explicit stencil-mask image XObject (`Mask`).
    Stencil(ObjectId),
    /// A soft-mask image XObject (`SMask`), validated as a DeviceGray image
    /// without masks of its own, and its `Matte` (11.6.5.3). The caller
    /// checks the Matte's length against the image's colour space and the
    /// mask's dimensions against the image's.
    Soft {
        /// The mask image.
        object: ObjectId,
        /// The colour the image was premultiplied with, if any.
        matte: Option<Vec<f64>>,
    },
    /// Opacity carried in JPEG 2000 data (`SMaskInData` 1 or 2, 7.4.9); the
    /// decoded image holds it as [`crate::MaskKind::Embedded`].
    Embedded {
        /// Whether the colour samples are premultiplied (`SMaskInData` 2).
        premultiplied: bool,
    },
}

/// An image whose dictionary has been read and validated and whose data is
/// held undecoded, apart from general-purpose filters. It does not refer to
/// the document, so it can be kept in a recorded scene.
///
/// It exposes what interpretation needs to describe the image without
/// decoding it: dimensions, components and precision, the colour space value
/// (or the JPEG 2000 container's colour), the decode array, interpolation,
/// the rendering intent and the mask (colour key, stencil or soft-mask
/// image with its Matte, embedded alpha).
#[derive(Debug, Clone)]
pub struct ImageSource {
    id: ImageId,
    /// Where errors and repairs without a position of their own are placed;
    /// `None` for an inline image, whose positions stay relative to its data.
    origin: Option<Position>,
    header: Arc<Header>,
    data: Arc<[u8]>,
    drawn: bool,
    recovery: Recovery,
}

/// How many samples a decode should produce.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Reduce {
    /// Every sample, exactly.
    #[default]
    Full,
    /// Exact block means where they are exact and full samples elsewhere:
    /// the result is the same as decoding fully and averaging. Decoding is
    /// currently full everywhere: the mean of a JPEG block's decoded samples
    /// cannot be taken from its DC term exactly, because each sample is
    /// rounded to a byte.
    ExactMeans {
        /// The fewest samples per output pixel along x that painting needs.
        x: u32,
        /// The fewest samples per output pixel along y that painting needs.
        y: u32,
    },
    /// Reconstruction near the drawn size: JPEG cells per eight-sample block
    /// axis (1, 2, 3, 4 or 6), from [`ImageSource::drawn`], interleaved or,
    /// in the [`Layout::Native`] layout, in component planes of cells. Only
    /// under the `reduced_image_decode` fast parameter; not within the
    /// one-level contract. JPEG 2000 resolution levels are not supported by
    /// the decoder, so JPEG 2000 images decode fully.
    Drawn {
        /// Samples kept per eight-sample block axis.
        cells: u8,
    },
}

/// How decoded samples are laid out.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Layout {
    /// Interleaved samples, byte for byte as the decoders define them.
    #[default]
    Interleaved,
    /// The decoders' native layouts where they apply: a baseline YCbCr JPEG
    /// in its component planes ([`crate::Samples::Planar`]) and one-bit,
    /// one-component samples packed ([`crate::Samples::Packed`]). Values are
    /// those of the interleaved layout.
    Native,
}

/// How decoded samples relate to the image's rows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Orientation {
    /// Rows as the image stores them.
    #[default]
    Rows,
    /// Rows and columns exchanged: sample (`x`, `y`) of the decoded image
    /// is sample (`y`, `x`) of the image, as
    /// [`DecodedImage::transposed`](crate::DecodedImage::transposed) gives
    /// it. A JPEG decoded into component planes stores its blocks
    /// exchanged as it decodes them, which costs no extra pass.
    Transposed,
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
    /// The samples painting reads; rows and blocks outside may be skipped
    /// and then hold unspecified values. `None` asks for the whole image.
    /// The decoders currently decode every sample: errors and repairs must
    /// not depend on the region, so the entropy-coded data, most of a
    /// JPEG's decoding, is read in full either way, and a whole decode in
    /// the store serves every region of a page.
    pub region: Option<SampleRect>,
    /// The sample layout.
    pub layout: Layout,
    /// Whether rows and columns are exchanged.
    pub orientation: Orientation,
}

impl DecodeRequest {
    /// A request for every sample, interleaved.
    pub const FULL: Self = Self {
        reduce: Reduce::Full,
        region: None,
        layout: Layout::Interleaved,
        orientation: Orientation::Rows,
    };

    /// A request for `reduce` over `region` in `layout`.
    pub fn new(reduce: Reduce, region: Option<SampleRect>, layout: Layout) -> Self {
        Self {
            reduce,
            region,
            layout,
            orientation: Orientation::Rows,
        }
    }

    /// The same request with samples in `orientation`.
    #[must_use]
    pub fn with_orientation(self, orientation: Orientation) -> Self {
        Self {
            orientation,
            ..self
        }
    }
}

/// The result of a decode: the samples and the repairs made while decoding,
/// which the caller reports in the image's object.
#[derive(Debug)]
#[non_exhaustive]
pub struct Decoded {
    /// The image.
    pub image: DecodedImage,
    /// Repairs made while decoding the data, positioned in the image's
    /// stream and attributed to its object (relative to the data for an
    /// inline image).
    pub diagnostics: Vec<Diagnostic>,
}

impl From<DecodedImage> for Decoded {
    /// A decode that made no repairs.
    fn from(image: DecodedImage) -> Self {
        Self {
            image,
            diagnostics: Vec::new(),
        }
    }
}

impl ImageSource {
    /// Reads and validates an image XObject (8.9.5) without decoding its
    /// samples: its dictionary, mask and data up to its codec. Repairs of
    /// the dictionary and of the general-purpose filters are reported to
    /// `document`.
    ///
    /// # Errors
    ///
    /// Returns `Syntax` for a malformed dictionary, `Unsupported` for
    /// unsupported codecs or features, and a `Limit` error for an image
    /// beyond `document.limits()`.
    pub fn from_xobject(document: &Document, image: &IndirectObject) -> Result<Self> {
        let mut diagnostics = Diagnostics::new(document.recovery());
        let origin = image.source().position();
        let id = image.id();
        let result = header::xobject(document, image, &mut diagnostics);
        document.report(
            diagnostics
                .take()
                .into_iter()
                .map(|diagnostic| place_diagnostic(diagnostic, id, origin)),
        );
        let (header, data) = result.map_err(|error| place_error(error, id, origin))?;
        let drawn = image
            .dictionary()
            .is_some_and(|dictionary| header::drawn(document, dictionary));
        Ok(Self {
            id: ImageId::Object(id),
            origin: Some(origin),
            header: Arc::new(header),
            data: data.into(),
            drawn,
            recovery: document.recovery(),
        })
    }

    /// Reads and validates an inline image (8.9.7) given its dictionary
    /// (abbreviated keys allowed), its data and its identity, up to its
    /// codec. Positions stay relative to the data, for the caller to place.
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
        let mut diagnostics = Diagnostics::new(document.recovery());
        let result = header::inline(document, resources, dictionary, data, &mut diagnostics);
        document.report(diagnostics.take());
        let (header, data) = result.map_err(|error| error.at(Position::Buffer(0)))?;
        Ok(Self {
            id,
            origin: None,
            header: Arc::new(header),
            data: data.into(),
            drawn: false,
            recovery: document.recovery(),
        })
    }

    /// The identity.
    pub fn id(&self) -> ImageId {
        self.id
    }

    /// The recovery mode of the document the source was read from, which
    /// its decodes honour.
    pub fn recovery(&self) -> Recovery {
        self.recovery
    }

    /// Width in samples (`Width`).
    pub fn width(&self) -> u32 {
        self.header.width
    }

    /// Height in samples (`Height`).
    pub fn height(&self) -> u32 {
        self.header.height
    }

    /// Colour components per sample of the decoded image: 1 for a stencil
    /// mask, the colour space's for other images, and the container's
    /// colour channels for a JPEG 2000 image without a colour space.
    pub fn components(&self) -> u8 {
        self.header.components
    }

    /// The precision of the decoded colour component `index` (1, 2, 4, 8
    /// or 16; 1–16 for JPEG 2000; 8 for JPEG), or `None` beyond the
    /// components.
    pub fn component_bits(&self, index: usize) -> Option<u8> {
        self.header.bits.get(index).copied()
    }

    /// The common precision of the decoded components, or 0 when a JPEG 2000
    /// image's components differ.
    pub fn bits_per_component(&self) -> u8 {
        match self.header.bits.as_slice() {
            [first, rest @ ..] if rest.iter().all(|bits| bits == first) => *first,
            [] => self.header.declared_bits,
            _ => 0,
        }
    }

    /// How the samples are interpreted: a resolved `ColorSpace` value, the
    /// JPEG 2000 container's colour, or a stencil mask and its polarity.
    pub fn kind(&self) -> &ImageKind {
        &self.header.kind
    }

    /// The resolved `ColorSpace` value, if the dictionary has one (a JPEG
    /// 2000 container's enumerated device space is given as its name).
    pub fn color_space(&self) -> Option<&Value> {
        match &self.header.kind {
            ImageKind::Color { color_space } => color_space.as_ref(),
            ImageKind::EmbeddedColor { .. } | ImageKind::Stencil { .. } => None,
        }
    }

    /// The validated `Decode` array, two values per component, if present
    /// (a JPEG 2000 image without a colour space ignores it).
    pub fn decode_array(&self) -> Option<&[f64]> {
        self.header.decode.as_deref()
    }

    /// `Interpolate`.
    pub fn interpolate(&self) -> bool {
        self.header.interpolate
    }

    /// The image's own `Intent` entry, resolved, if any; stencil masks have
    /// none (Table 87). It is not validated: a value that is not a name is
    /// the caller's error to report.
    pub fn intent(&self) -> Option<&Value> {
        self.header.intent.as_ref()
    }

    /// The mask.
    pub fn mask(&self) -> &SourceMask {
        &self.header.mask
    }

    /// Bytes this source retains.
    pub fn retained_bytes(&self) -> usize {
        let header = &self.header;
        let space = match &header.kind {
            ImageKind::Color {
                color_space: Some(value),
            } => value.retained_bytes(),
            ImageKind::EmbeddedColor {
                color: crate::jpx::JpxColor::Icc(profile),
            } => profile.capacity(),
            _ => 0,
        };
        let mask = match &header.mask {
            SourceMask::ColorKey(ranges) => ranges.capacity() * size_of::<[u16; 2]>(),
            SourceMask::Soft {
                matte: Some(matte), ..
            } => matte.capacity() * size_of::<f64>(),
            _ => 0,
        };
        let codec = match &header.codec {
            Codec::Dct(Some(params)) | Codec::Ccitt(Some(params)) => params.retained_bytes(),
            Codec::Jbig2(Some(globals)) => globals.len(),
            _ => 0,
        };
        [
            size_of::<Self>(),
            size_of::<Header>(),
            self.data.len(),
            space,
            mask,
            codec,
            header.bits.capacity(),
            header.decode.as_ref().map_or(0, |d| d.capacity() * 8),
        ]
        .into_iter()
        .fold(0, usize::saturating_add)
    }

    /// The reduction near the drawn size for an image drawn with
    /// `samples_per_pixel` source samples per device pixel along its less
    /// minified axis, under the `reduced_image_decode` fast parameter: the
    /// fewest JPEG cells per block axis, of 1, 2, 3, 4 and 6, that leave at
    /// least two and a half cells per device pixel, for an opaque DeviceGray
    /// or DeviceRGB `DCTDecode` image XObject without `Mask`, `SMask` or
    /// `Decode`; [`Reduce::Full`] otherwise.
    pub fn drawn(&self, samples_per_pixel: f64) -> Reduce {
        if !self.drawn {
            return Reduce::Full;
        }
        [1, 2, 3, 4, 6]
            .into_iter()
            .find(|&cells| samples_per_pixel * f64::from(cells) >= 20.0)
            .map_or(Reduce::Full, |cells| Reduce::Drawn { cells })
    }

    /// An upper bound of the bytes (decoded samples and the codec's working
    /// storage) and `decode_work` units (one per coefficient, sample and
    /// entropy symbol) a decode of `request` uses, for reservation before
    /// decoding, for data the codec accepts. The decoders' own budgets
    /// bound hostile data.
    pub fn decode_cost(&self, request: &DecodeRequest) -> (usize, usize) {
        let header = &self.header;
        let (width, height) = self.decoded_size(request);
        let pixels = u64::from(width) * u64::from(height);
        let components = u64::from(header.components.max(1));
        let wide = header.bits.iter().any(|&bits| bits > 8);
        let values = pixels.saturating_mul(components);
        let output = values.saturating_mul(if wide { 2 } else { 1 });
        let alpha = if matches!(header.mask, SourceMask::Embedded { .. }) {
            pixels.saturating_mul(2)
        } else {
            0
        };
        let source = u64::from(header.width) * u64::from(header.height);
        let source_values = source.saturating_mul(components.max(4));
        let (working, work) = match header.codec {
            Codec::None => (0, values),
            // Coefficients or native planes with block padding, and the
            // final interleave.
            Codec::Dct(_) => (
                source_values.saturating_mul(4 * 6),
                source_values.saturating_mul(4),
            ),
            Codec::Ccitt(_) | Codec::Jbig2(_) => {
                (source.saturating_mul(2), source.saturating_mul(4))
            }
            Codec::Jpx(_) => (
                source_values.saturating_mul(16),
                source_values.saturating_mul(64),
            ),
        };
        // Samples exchanged after decoding are held twice for a moment.
        let exchange = match request.orientation {
            Orientation::Transposed => output.saturating_add(alpha),
            Orientation::Rows => 0,
        };
        let bytes = output
            .saturating_add(alpha)
            .saturating_add(working)
            .saturating_add(exchange);
        (saturate(bytes), saturate(work))
    }

    /// Decodes the samples `request` asks for: every sample exactly unless
    /// the request reduces it, with the decoded image's painting entries.
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
        let mut diagnostics = Diagnostics::new(recovery);
        let result = codec::decode(
            &self.header,
            &self.data,
            (self.layout(request), request.orientation),
            limits,
            &mut diagnostics,
        );
        let diagnostics = diagnostics.take();
        match (self.id, self.origin) {
            (ImageId::Object(id), Some(origin)) => Ok(Decoded {
                image: result.map_err(|error| place_error(error, id, origin))?,
                diagnostics: diagnostics
                    .into_iter()
                    .map(|diagnostic| place_diagnostic(diagnostic, id, origin))
                    .collect(),
            }),
            _ => Ok(Decoded {
                image: result.map_err(|error| error.at(Position::Buffer(0)))?,
                diagnostics,
            }),
        }
    }

    /// The sample layout a request decodes into.
    fn layout(&self, request: &DecodeRequest) -> SampleLayout {
        match request.reduce {
            Reduce::Drawn { cells }
                if self.drawn
                    && matches!(self.header.codec, Codec::Dct(_))
                    && matches!(cells, 1 | 2 | 3 | 4 | 6) =>
            {
                match request.layout {
                    Layout::Interleaved => SampleLayout::Cells(cells),
                    Layout::Native => SampleLayout::DeferredCells(cells),
                }
            }
            _ => match request.layout {
                Layout::Interleaved => SampleLayout::Interleaved,
                Layout::Native => SampleLayout::Deferred,
            },
        }
    }

    /// The stored dimensions of a decode of `request`.
    fn decoded_size(&self, request: &DecodeRequest) -> (u32, u32) {
        match self.layout(request) {
            SampleLayout::Cells(cells) | SampleLayout::DeferredCells(cells) => (
                crate::dct::scaled_length(self.header.width, cells),
                crate::dct::scaled_length(self.header.height, cells),
            ),
            SampleLayout::Interleaved | SampleLayout::Deferred => {
                (self.header.width, self.header.height)
            }
        }
    }
}

fn saturate(value: u64) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

/// Places a decoder's buffer offset in the image stream and an unknown
/// position at the image object, attributed to it, as the XObject entry
/// point does.
fn place_diagnostic(diagnostic: Diagnostic, id: ObjectId, origin: Position) -> Diagnostic {
    diagnostic
        .in_object(id)
        .within(Position::Stream {
            object: id,
            offset: 0,
        })
        .at(origin)
}

/// As [`place_diagnostic`] for an error.
fn place_error(error: Error, id: ObjectId, origin: Position) -> Error {
    error
        .in_object(id)
        .within(Position::Stream {
            object: id,
            offset: 0,
        })
        .at(origin)
}
