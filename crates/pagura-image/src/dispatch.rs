//! Image codec dispatch and cross-checking of dictionary and stream metadata.
use crate::{DecodedImage, ImageKind, MaskKind, Samples, ccitt, dct, jbig2, parameters as p};
use pagura_codec::ImageCodec;
use pagura_core::{Dictionary, Error, Result, Value};
use pagura_document::Document;

type Codec = (ImageCodec, Option<Dictionary>);

pub(crate) fn decode(
    document: &Document,
    dict: &Dictionary,
    data: &[u8],
    codec: Option<&Codec>,
) -> Result<DecodedImage> {
    let (width, height) = p::dimensions(dict)?;
    let limits = document.limits();
    p::budget(width, height, 1, 1, &limits)?;
    let stencil = p::boolean(dict, b"ImageMask", false)?;
    let (space, components) = if stencil {
        (None, 1)
    } else if let Some(space) = dict.get(b"ColorSpace").filter(|v| !v.is_null()) {
        let (value, n) = p::resolve_space(document, space)?;
        (Some(value), n)
    } else if matches!(codec, Some((ImageCodec::Jpx, _))) {
        (None, 0)
    } else {
        return Err(Error::syntax("missing image ColorSpace"));
    };
    let bits = if matches!(codec, Some((ImageCodec::Jpx, _))) {
        1
    } else {
        u8::try_from(p::integer(dict, b"BitsPerComponent", i64::from(stencil))?)
            .ok()
            .filter(|b| matches!(b, 1 | 2 | 4 | 8 | 16))
            .ok_or(Error::syntax("invalid image BitsPerComponent"))?
    };
    if !matches!(codec, Some((ImageCodec::Jpx, _))) {
        p::budget(width, height, components, bits, &limits)?;
    }
    let mut image = DecodedImage {
        width,
        height,
        components,
        bits_per_component: 1,
        samples: Samples::Bytes(Vec::new()),
        kind: if stencil {
            ImageKind::Stencil { inverted: false }
        } else {
            ImageKind::Color { color_space: space }
        },
        decode: None,
        mask: MaskKind::None,
        interpolate: false,
    };
    let params = codec.and_then(|(_, params)| params.as_ref());
    match codec.map(|(codec, _)| codec) {
        None => {
            image.bits_per_component = bits;
            image.samples = p::unpack(data, width, height, components, bits, &limits)?;
        }
        Some(ImageCodec::Dct) => {
            let transform = params
                .and_then(|dict| dict.get(b"ColorTransform"))
                .map(|v| {
                    v.as_integer()
                        .ok_or(Error::syntax("invalid DCT ColorTransform"))
                })
                .transpose()?;
            let decoded = dct::decode(data, transform, &limits)?;
            if decoded.width != width
                || decoded.height != height
                || decoded.components != components
                || bits != 8
            {
                return Err(Error::syntax(
                    "JPEG metadata disagrees with image dictionary",
                ));
            }
            image.bits_per_component = 8;
            image.samples = Samples::Bytes(decoded.samples);
        }
        Some(ImageCodec::CcittFax) => {
            if bits != 1 || components != 1 {
                return Err(Error::syntax("CCITT requires one-bit components"));
            }
            let params = ccitt::CcittParams::from_dictionary(params)?;
            if params.columns != width {
                return Err(Error::syntax("CCITT Columns disagrees with image Width"));
            }
            let decoded = ccitt::decode(data, &params, &limits)?;
            image.samples = p::unpack(&decoded, width, height, 1, 1, &limits)?;
        }
        Some(ImageCodec::Jbig2) => {
            if bits != 1 || components != 1 {
                return Err(Error::syntax("JBIG2 requires one-bit components"));
            }
            image.samples = decode_jbig2(document, dict, data, params, width, height)?;
        }
        Some(ImageCodec::Jpx) => crate::jpx_image::decode(data, dict, &mut image, &limits)?,
    }
    let mut dictionary = dict.clone();
    if matches!(codec, Some((ImageCodec::Jpx, _)))
        && dict.get(b"ColorSpace").is_none_or(Value::is_null)
        && !stencil
    {
        dictionary.remove(b"Decode");
    }
    p::finish(document, &dictionary, image)
}

fn decode_jbig2(
    document: &Document,
    dict: &Dictionary,
    data: &[u8],
    params: Option<&Dictionary>,
    width: u32,
    height: u32,
) -> Result<Samples> {
    // Preserve identity for stream-valued parameters using the original
    // dictionary; resolved filter parameters hold only object values.
    let original = dict
        .get(b"DecodeParms")
        .map(|v| document.resolve(v))
        .transpose()?;
    let original = match original {
        Some(Value::Array(values)) => values.last().map(|v| document.resolve(v)).transpose()?,
        value => value,
    };
    let globals = original
        .and_then(Value::as_dictionary)
        .or(params)
        .and_then(|p| p.get(b"JBIG2Globals"))
        .map(|v| {
            let id = v
                .as_reference()
                .ok_or(Error::syntax("JBIG2Globals must reference a stream"))?;
            document.stream_data(document.object(id)?)
        })
        .transpose()?;
    let decoded = jbig2::decode(data, globals.as_deref(), &document.limits())?;
    if decoded.width != width || decoded.height != height {
        return Err(Error::syntax(
            "JBIG2 dimensions disagree with image dictionary",
        ));
    }
    // JBIG2 black=1; PDF default image samples black=0 (7.4.7).
    let mut inverted = decoded.data;
    for byte in &mut inverted {
        *byte = !*byte;
    }
    let count = p::budget(width, height, 1, 1, &document.limits())?;
    if count
        .checked_add(inverted.len())
        .is_none_or(|v| v > document.limits().decoded_bytes)
    {
        return Err(Error::limit("JBIG2 adapter sample budget exceeded"));
    }
    p::unpack(&inverted, width, height, 1, 1, &document.limits())
}
