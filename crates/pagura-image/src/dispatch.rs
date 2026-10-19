//! Image codec dispatch, cross-checking of dictionary and stream metadata,
//! and the assembly of the decoded image (8.9.5, Table 87).
use crate::adapter::{self, Pixels};
use crate::image::Parts;
use crate::{DecodedImage, ImageAlpha, ImageKind, MaskKind, Precision, parameters as p};
use pagura_codec::ImageCodec;
use pagura_core::{Diagnostics, Dictionary, Error, Limits, Result, Value};
use pagura_document::Document;

/// The trailing image codec of a filter chain and its parameters.
#[derive(Clone, Copy)]
pub(crate) struct Codec<'a> {
    pub(crate) codec: ImageCodec,
    pub(crate) params: Option<&'a Dictionary>,
}

/// What `/ImageMask` and `/ColorSpace` make of the samples.
enum Interpretation {
    Stencil,
    Color {
        space: Value,
        components: u8,
    },
    /// JPEG 2000 without `/ColorSpace`: the container decides (7.4.9).
    Container,
}

impl Interpretation {
    const fn components(&self) -> Option<u8> {
        match self {
            Self::Stencil => Some(1),
            Self::Color { components, .. } => Some(*components),
            Self::Container => None,
        }
    }

    /// Whether the space is `/Indexed` (8.6.6.3).
    fn indexed(&self) -> bool {
        matches!(self, Self::Color { space: Value::Array(space), .. }
            if space.first().and_then(Value::as_name).is_some_and(|name| name == b"Indexed"))
    }
}

/// What a codec adapter decoded.
struct Decoded {
    pixels: Pixels,
    /// A JPEG 2000 opacity plane requested by `/SMaskInData`.
    alpha: Option<ImageAlpha>,
    /// A JPEG 2000 container's color, for an image without `/ColorSpace`.
    container: Option<ImageKind>,
}

impl From<Pixels> for Decoded {
    fn from(pixels: Pixels) -> Self {
        Self {
            pixels,
            alpha: None,
            container: None,
        }
    }
}

/// The image a dictionary declares, for the codec adapters.
struct Image<'a> {
    document: &'a Document,
    dict: &'a Dictionary,
    width: u32,
    height: u32,
    interpretation: Interpretation,
    limits: Limits,
}

impl Image<'_> {
    /// Color components the dictionary declares; one for a stencil and for
    /// a JPEG 2000 image whose container decides.
    fn components(&self) -> u8 {
        self.interpretation.components().unwrap_or(1)
    }

    /// `/BitsPerComponent` of an image that is not JPEG 2000 (Table 87),
    /// checked against the sample budget.
    fn bits(&self) -> Result<u8> {
        let stencil = matches!(self.interpretation, Interpretation::Stencil);
        let bits = u8::try_from(p::integer(
            self.dict,
            b"BitsPerComponent",
            i64::from(stencil),
        )?)
        .ok()
        .filter(|b| matches!(b, 1 | 2 | 4 | 8 | 16))
        .ok_or(Error::syntax("invalid image BitsPerComponent"))?;
        p::budget(
            self.width,
            self.height,
            self.components(),
            bits,
            &self.limits,
        )?;
        Ok(bits)
    }

    /// Checks that a bilevel codec's image has one 1-bit component.
    fn bilevel(&self, message: &'static str) -> Result<()> {
        if self.bits()? == 1 && self.components() == 1 {
            Ok(())
        } else {
            Err(Error::syntax(message))
        }
    }
}

pub(crate) fn decode(
    document: &Document,
    dict: &Dictionary,
    data: &[u8],
    codec: Option<Codec<'_>>,
    diagnostics: &mut Diagnostics,
) -> Result<DecodedImage> {
    let (width, height) = p::dimensions(dict)?;
    let limits = document.limits();
    p::budget(width, height, 1, 1, &limits)?;
    let jpx = codec.as_ref().is_some_and(|c| c.codec == ImageCodec::Jpx);
    let interpretation = if p::boolean(dict, b"ImageMask", false)? {
        Interpretation::Stencil
    } else if let Some(space) = dict.get(b"ColorSpace").filter(|v| !v.is_null()) {
        let (space, components) = p::resolve_space(document, space)?;
        Interpretation::Color { space, components }
    } else if jpx {
        Interpretation::Container
    } else {
        return Err(Error::syntax("missing image ColorSpace"));
    };
    let image = Image {
        document,
        dict,
        width,
        height,
        interpretation,
        limits,
    };
    let decoded = decode_data(&image, data, codec, diagnostics)?;
    assemble(image, decoded)
}

/// Decodes the samples with the image's codec, or unpacks them.
fn decode_data(
    image: &Image<'_>,
    data: &[u8],
    codec: Option<Codec<'_>>,
    diagnostics: &mut Diagnostics,
) -> Result<Decoded> {
    let (width, height, limits) = (image.width, image.height, &image.limits);
    let Some(Codec { codec, params }) = codec else {
        let bits = image.bits()?;
        let samples = p::unpack_with_diagnostics(
            data,
            width,
            height,
            image.components(),
            bits,
            limits,
            diagnostics,
        )?;
        return Ok(Pixels {
            components: image.components(),
            precision: Precision::Uniform(bits),
            samples,
        }
        .into());
    };
    Ok(match codec {
        ImageCodec::Dct => {
            let declared = adapter::dct::Declared {
                width,
                height,
                components: image.components(),
                bits: image.bits()?,
            };
            adapter::dct::decode(data, params, &declared, limits, diagnostics)?.into()
        }
        ImageCodec::CcittFax => {
            image.bilevel("CCITT requires one-bit components")?;
            adapter::fax::decode(data, params, width, height, limits, diagnostics)?.into()
        }
        ImageCodec::Jbig2 => {
            image.bilevel("JBIG2 requires one-bit components")?;
            adapter::jbig2::decode(image.document, image.dict, data, params, width, height)?.into()
        }
        ImageCodec::Jpx => {
            let declared = adapter::jpx::Declared {
                width,
                height,
                components: image.interpretation.components(),
                indexed: image.interpretation.indexed(),
            };
            let adapted = adapter::jpx::decode(data, image.dict, declared, limits, diagnostics)?;
            Decoded {
                pixels: adapted.pixels,
                alpha: adapted.alpha,
                container: adapted.kind,
            }
        }
    })
}

/// Adds the dictionary's painting parameters to the decoded samples.
fn assemble(image: Image<'_>, decoded: Decoded) -> Result<DecodedImage> {
    let Image {
        document,
        dict,
        width,
        height,
        interpretation,
        ..
    } = image;
    let Decoded {
        pixels,
        alpha,
        container,
    } = decoded;
    // A JPEG 2000 color image without /ColorSpace ignores /Decode (Table 87).
    let decode = if matches!(interpretation, Interpretation::Container) {
        None
    } else {
        p::decode_array(document, dict, pixels.components)?
    };
    let stencil = matches!(interpretation, Interpretation::Stencil);
    let kind = match interpretation {
        Interpretation::Stencil => ImageKind::Stencil {
            inverted: stencil_inverted(dict, &pixels, decode.as_deref())?,
        },
        Interpretation::Color { space, .. } => ImageKind::Color { color_space: space },
        Interpretation::Container => container.ok_or(Error::syntax(
            "JPX without dictionary ColorSpace requires container color information",
        ))?,
    };
    let mask = match alpha {
        Some(alpha) => MaskKind::Embedded(alpha),
        None if stencil => MaskKind::None,
        None => crate::mask::read(document, dict, pixels.components, &pixels.precision)?,
    };
    Ok(DecodedImage::assemble(Parts {
        width,
        height,
        pixels,
        kind,
        decode,
        mask,
        interpolate: p::boolean(dict, b"Interpolate", false)?,
    }))
}

/// The stencil constraints of Table 87 and the sense of `/Decode` (8.9.6.2).
fn stencil_inverted(dict: &Dictionary, pixels: &Pixels, decode: Option<&[f64]>) -> Result<bool> {
    if pixels.precision != Precision::Uniform(1)
        || [b"ColorSpace".as_slice(), b"Mask", b"SMask"]
            .into_iter()
            .any(|key| dict.get(key).is_some_and(|v| !v.is_null()))
    {
        return Err(Error::syntax("invalid stencil image dictionary"));
    }
    match decode {
        None | Some([0.0, 1.0]) => Ok(false),
        Some([1.0, 0.0]) => Ok(true),
        _ => Err(Error::syntax("invalid stencil Decode")),
    }
}
