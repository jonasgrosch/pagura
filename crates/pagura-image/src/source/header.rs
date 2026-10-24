//! An image dictionary read and validated without decoding samples (8.9.5,
//! 8.9.6, 8.9.7, Tables 87–92): the checks a decode makes before and after
//! decoding that need no samples, in their order.

use super::SourceMask;
use crate::jpx_image::Jpx;
use crate::{ImageKind, MaskKind, dispatch, filter_rules, inline, mask, parameters as p, stream};
use pagura_codec::{Filter, FilterChain, ImageCodec};
use pagura_core::{Diagnostic, Diagnostics, Dictionary, Error, Name, Position, Result, Value};
use pagura_document::{Document, IndirectObject};
use std::sync::Arc;

/// How the data after the general-purpose filters becomes samples.
#[derive(Debug, Clone)]
pub(crate) enum Codec {
    /// The data are the packed samples.
    None,
    /// `DCTDecode` with its parameters.
    Dct(Option<Dictionary>),
    /// `CCITTFaxDecode` with its parameters.
    Ccitt(Option<Dictionary>),
    /// `JBIG2Decode` with the data of its `JBIG2Globals`.
    Jbig2(Option<Arc<[u8]>>),
    /// `JPXDecode`.
    Jpx(Jpx),
}

/// What decoding needs and interpretation reads, without samples.
#[derive(Debug, Clone)]
pub(crate) struct Header {
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// Colour components the dictionary declares (1 for a stencil; 0 for
    /// JPXDecode without a colour space): decoding starts from this.
    pub(crate) declared_components: u8,
    /// `BitsPerComponent` as decoding reads it (1 for JPXDecode).
    pub(crate) declared_bits: u8,
    /// Colour components of the decoded samples.
    pub(crate) components: u8,
    /// Precision of each decoded colour component.
    pub(crate) bits: Vec<u8>,
    /// The interpretation decoding starts from.
    pub(crate) initial: ImageKind,
    /// The interpretation of the decoded samples.
    pub(crate) kind: ImageKind,
    pub(crate) decode: Option<Vec<f64>>,
    pub(crate) interpolate: bool,
    pub(crate) mask: SourceMask,
    pub(crate) intent: Option<Value>,
    pub(crate) codec: Codec,
    /// Whether a stencil's precision is known only after decoding.
    pub(crate) stencil_after_decode: bool,
}

impl Header {
    /// The masking of the decoded image, as decoding reports it, apart from
    /// embedded opacity, which only decoding supplies.
    pub(crate) fn mask_kind(&self) -> MaskKind {
        match &self.mask {
            SourceMask::None | SourceMask::Embedded { .. } => MaskKind::None,
            SourceMask::ColorKey(ranges) => {
                MaskKind::ColorKey(ranges.iter().map(|&[low, high]| (low, high)).collect())
            }
            SourceMask::Stencil(id) => MaskKind::Stencil(*id),
            SourceMask::Soft { object, .. } => MaskKind::Soft(*object),
        }
    }

    /// Whether the samples are a stencil mask.
    pub(crate) fn stencil(&self) -> bool {
        matches!(self.initial, ImageKind::Stencil { .. })
    }
}

/// Reads an image XObject's dictionary and data before decoding, returning
/// the header and the data after decryption and the general-purpose
/// filters.
pub(crate) fn xobject(
    document: &Document,
    image: &IndirectObject,
    diagnostics: &mut Diagnostics,
) -> Result<(Header, Vec<u8>)> {
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
            Diagnostic::new(
                crate::tags::XOBJECT_TYPE,
                "Subtype Image identifies XObject; invalid Type ignored (8.9.5 Table 87)",
            )
            .at(Position::Buffer(0)),
        )?;
    }
    let dict = p::resolved_dictionary(document, dictionary, diagnostics)?;
    filter_rules::validate(document, &dict, false, diagnostics)?;
    let (width, height) = p::dimensions(&dict)?;
    p::budget(width, height, 1, 1, &document.limits())?;
    let stream = stream::read(document, image, diagnostics)?;
    let header = read(
        document,
        &dict,
        &stream.data,
        stream.codec,
        Some(dictionary),
    )?;
    Ok((header, stream.data.into_owned()))
}

/// Reads an inline image's dictionary and data before decoding.
pub(crate) fn inline(
    document: &Document,
    resources: &Dictionary,
    dictionary: &Dictionary,
    data: &[u8],
    diagnostics: &mut Diagnostics,
) -> Result<(Header, Vec<u8>)> {
    let mut dict = inline::expanded(dictionary)?;
    if p::stencil_space(document, &dict, diagnostics)? {
        dict.remove(b"ColorSpace");
    }
    filter_rules::inline_space(dict.get(b"ColorSpace"))?;
    filter_rules::validate(document, &dict, true, diagnostics)?;
    if let Some(Value::Name(name)) = dict.get(b"ColorSpace")
        && !matches!(
            name.as_bytes(),
            b"DeviceGray" | b"DeviceRGB" | b"DeviceCMYK"
        )
    {
        let spaces = resources
            .get(b"ColorSpace")
            .ok_or(Error::syntax("missing inline colour resource"))?;
        let space = document
            .resolve_dictionary(spaces)?
            .get(name.as_bytes())
            .ok_or(Error::syntax("undefined inline colour space"))?;
        dict.insert(Name::new(b"ColorSpace"), document.resolve(space)?.clone());
    }
    let (width, height) = p::dimensions(&dict)?;
    p::budget(width, height, 1, 1, &document.limits())?;
    let mut chain = FilterChain::from_entries(dict.get(b"Filter"), dict.get(b"DecodeParms"))?;
    if chain.filters().iter().any(|f| {
        matches!(
            f,
            Filter::Crypt(_)
                | Filter::Image {
                    codec: ImageCodec::Jbig2 | ImageCodec::Jpx,
                    ..
                }
        )
    }) {
        return Err(Error::syntax("filter is forbidden in inline images"));
    }
    let codec = chain.split_image();
    let data = chain.decode(data, document.limits().decoded_bytes, diagnostics)?;
    let header = read(document, &dict, &data, codec, None)?;
    Ok((header, data.into_owned()))
}

/// The checks of image dispatch and of finishing a decoded image that need
/// no samples. `original` is an XObject's unresolved dictionary, which
/// keeps the identity of a `JBIG2Globals` stream.
fn read(
    document: &Document,
    dict: &Dictionary,
    data: &[u8],
    codec: Option<(ImageCodec, Option<Dictionary>)>,
    original: Option<&Dictionary>,
) -> Result<Header> {
    let limits = document.limits();
    let jpx = matches!(codec, Some((ImageCodec::Jpx, _)));
    let (width, height) = p::dimensions(dict)?;
    p::budget(width, height, 1, 1, &limits)?;
    let stencil = p::boolean(dict, b"ImageMask", false)?;
    let (space, components) = if stencil {
        (None, 1)
    } else if let Some(space) = dict.get(b"ColorSpace").filter(|v| !v.is_null()) {
        let (value, n) = p::resolve_space(document, space)?;
        (Some(value), n)
    } else if jpx {
        (None, 0)
    } else {
        return Err(Error::syntax("missing image ColorSpace"));
    };
    let bits = if jpx {
        1
    } else {
        u8::try_from(p::integer(dict, b"BitsPerComponent", i64::from(stencil))?)
            .ok()
            .filter(|b| matches!(b, 1 | 2 | 4 | 8 | 16))
            .ok_or(Error::syntax("invalid image BitsPerComponent"))?
    };
    if !jpx {
        p::budget(width, height, components, bits, &limits)?;
    }
    let initial = if stencil {
        ImageKind::Stencil { inverted: false }
    } else {
        ImageKind::Color { color_space: space }
    };
    let mut header = Header {
        width,
        height,
        declared_components: components,
        declared_bits: bits,
        components,
        bits: vec![bits; usize::from(components)],
        kind: initial.clone(),
        initial,
        decode: None,
        interpolate: false,
        mask: SourceMask::None,
        intent: None,
        codec: Codec::None,
        stencil_after_decode: false,
    };
    header.codec = codec_of(document, dict, data, codec, original, &mut header)?;
    finish(document, dict, &mut header)?;
    Ok(header)
}

/// The codec and the checks it makes before decoding, with the precision
/// and channels of the samples it will produce.
fn codec_of(
    document: &Document,
    dict: &Dictionary,
    data: &[u8],
    codec: Option<(ImageCodec, Option<Dictionary>)>,
    original: Option<&Dictionary>,
    header: &mut Header,
) -> Result<Codec> {
    let one_bit = header.declared_bits == 1 && header.declared_components == 1;
    Ok(match codec {
        None => Codec::None,
        Some((ImageCodec::Dct, params)) => {
            dispatch::color_transform(params.as_ref())?;
            header.bits.fill(8);
            header.stencil_after_decode = true;
            Codec::Dct(params)
        }
        Some((ImageCodec::CcittFax, params)) => {
            if !one_bit {
                return Err(Error::syntax("CCITT requires one-bit components"));
            }
            Codec::Ccitt(params)
        }
        Some((ImageCodec::Jbig2, params)) => {
            if !one_bit {
                return Err(Error::syntax("JBIG2 requires one-bit components"));
            }
            let globals = match original {
                Some(original) => dispatch::jbig2_globals(document, original, params.as_ref())?
                    .map(|data| Arc::from(data.into_owned())),
                None => None,
            };
            Codec::Jbig2(globals)
        }
        Some((ImageCodec::Jpx, _)) => {
            let jpx = Jpx::read(data, dict, &header.initial)?;
            let description = jpx.describe(
                data,
                (header.width, header.height, header.declared_components),
                &document.limits(),
                document.recovery(),
            )?;
            header.components = u8::try_from(description.bits.len())
                .map_err(|_| Error::limit("too many JPX components"))?;
            header.bits = description.bits;
            header.stencil_after_decode = true;
            if let Some(kind) = description.kind {
                header.kind = kind;
            }
            if jpx.mode != 0 {
                header.mask = SourceMask::Embedded {
                    premultiplied: jpx.mode == 2,
                };
            }
            Codec::Jpx(jpx)
        }
    })
}

/// The checks and entries of finishing a decoded image (Decode,
/// Interpolate, stencil constraints and masks), the Matte of a soft mask
/// and the rendering intent.
fn finish(document: &Document, dict: &Dictionary, header: &mut Header) -> Result<()> {
    let removed = matches!(header.codec, Codec::Jpx(_))
        && dict.get(b"ColorSpace").is_none_or(Value::is_null)
        && !header.stencil();
    if !removed && let Some(value) = dict.get(b"Decode").filter(|v| !v.is_null()) {
        header.decode = Some(decode_array(document, value, header.components)?);
    }
    header.interpolate = p::boolean(dict, b"Interpolate", false)?;
    if header.stencil() {
        let bits_known = !header.stencil_after_decode;
        if (bits_known && header.bits.first() != Some(&1))
            || dict.get(b"ColorSpace").is_some_and(|v| !v.is_null())
            || dict.get(b"Mask").is_some_and(|v| !v.is_null())
            || dict.get(b"SMask").is_some_and(|v| !v.is_null())
        {
            return Err(Error::syntax("invalid stencil image dictionary"));
        }
        let inverted = match header.decode.as_deref() {
            None | Some([0.0, 1.0]) => false,
            Some([1.0, 0.0]) => true,
            _ => return Err(Error::syntax("invalid stencil Decode")),
        };
        header.kind = ImageKind::Stencil { inverted };
        return Ok(());
    }
    if !matches!(header.mask, SourceMask::Embedded { .. }) {
        let bits = header.bits.clone();
        header.mask = match mask::read_for(document, dict, header.components, &|component| {
            bits.get(component).copied()
        })? {
            MaskKind::ColorKey(ranges) => {
                SourceMask::ColorKey(ranges.into_iter().map(|(low, high)| [low, high]).collect())
            }
            MaskKind::Stencil(id) => SourceMask::Stencil(id),
            MaskKind::Soft(object) => SourceMask::Soft {
                object,
                matte: matte(document, object)?,
            },
            MaskKind::None | MaskKind::InData | MaskKind::Embedded(_) => SourceMask::None,
        };
    }
    header.intent = intent(document, dict)?;
    Ok(())
}

fn decode_array(document: &Document, value: &Value, components: u8) -> Result<Vec<f64>> {
    let decode = document
        .resolve(value)?
        .as_array()
        .ok_or(Error::syntax("Decode must be an array"))?;
    if decode.len() != usize::from(components) * 2 {
        return Err(Error::syntax("invalid Decode length"));
    }
    decode
        .iter()
        .map(|v| {
            document
                .resolve(v)?
                .as_number()
                .filter(|v| v.is_finite())
                .ok_or(Error::syntax("invalid Decode value"))
        })
        .collect()
}

/// The `Matte` of the soft-mask image `object` (11.6.5.3): numbers, whose
/// count and the mask's dimensions the caller checks against the image.
fn matte(document: &Document, object: pagura_core::ObjectId) -> Result<Option<Vec<f64>>> {
    let Some(value) = document
        .object(object)?
        .dictionary()
        .and_then(|d| d.get(b"Matte"))
    else {
        return Ok(None);
    };
    document
        .resolve(value)?
        .as_array()
        .ok_or(Error::syntax("invalid image Matte"))?
        .iter()
        .map(|v| {
            document
                .resolve(v)?
                .as_number()
                .ok_or(Error::syntax("invalid Matte component"))
        })
        .collect::<Result<Vec<_>>>()
        .map(Some)
}

/// The image's own `Intent` entry, resolved; a null entry is absent
/// (7.3.9). Decoding does not interpret it, so it is not validated here.
fn intent(document: &Document, dict: &Dictionary) -> Result<Option<Value>> {
    Ok(dict
        .get(b"Intent")
        .map(|value| document.resolve(value))
        .transpose()?
        .filter(|value| !value.is_null())
        .cloned())
}
