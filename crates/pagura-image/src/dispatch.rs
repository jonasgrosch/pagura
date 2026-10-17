//! Image codec dispatch and cross-checking of dictionary and stream metadata.
use crate::{DecodedImage, ImageKind, MaskKind, Samples, ccitt, dct, jbig2, jpx, parameters as p};
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
        kind: ImageKind::Color { color_space: space },
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
        Some(ImageCodec::Jpx) => decode_jpx(data, &mut image, &limits)?,
    }
    let mut dictionary = dict.clone();
    if matches!(codec, Some((ImageCodec::Jpx, _))) {
        let alpha = p::integer(dict, b"SMaskInData", 0)?;
        if !(0..=2).contains(&alpha) || (alpha != 0 && dict.contains_key(b"SMask")) {
            return Err(Error::syntax("invalid SMaskInData or simultaneous SMask"));
        }
        if alpha != 0 {
            return Err(Error::unsupported(
                crate::features::EMBEDDED_ALPHA,
                "embedded image alpha requires an alpha sample API",
            ));
        }
        if dict.get(b"ColorSpace").is_none_or(Value::is_null) && !stencil {
            dictionary.remove(b"Decode");
        }
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
    p::unpack(&inverted, width, height, 1, 1, &document.limits())
}

fn decode_jpx(data: &[u8], image: &mut DecodedImage, limits: &pagura_core::Limits) -> Result<()> {
    let decoded = jpx::decode(data, limits)?;
    if decoded.width != image.width || decoded.height != image.height {
        return Err(Error::syntax(
            "JPX dimensions disagree with image dictionary",
        ));
    }
    if decoded
        .alpha
        .is_some_and(|index| index >= decoded.components.len())
    {
        return Err(Error::syntax("invalid JPX alpha component index"));
    }
    // Table 87, SMaskInData=0: opacity is ignored, not painted as colour.
    let components: Vec<_> = decoded
        .components
        .iter()
        .enumerate()
        .filter(|(index, _)| Some(*index) != decoded.alpha)
        .map(|(_, c)| c)
        .collect();
    let first = components
        .first()
        .ok_or(Error::syntax("JPX has no components"))?;
    let n = u8::try_from(components.len()).map_err(|_| Error::limit("too many JPX components"))?;
    if image.components != 0 && image.components != n {
        return Err(Error::syntax(
            "JPX component count disagrees with colour space",
        ));
    }
    if !matches!(first.bits, 1 | 2 | 4 | 8 | 16)
        || components
            .iter()
            .any(|c| c.bits != first.bits || c.width != image.width || c.height != image.height)
    {
        return Err(Error::unsupported(
            crate::features::JPX_SAMPLE_LAYOUT,
            "JPX component precision or subsampling is not representable",
        ));
    }
    if let ImageKind::Color { color_space } = &mut image.kind
        && color_space.is_none()
    {
        *color_space = Some(match decoded.color {
            jpx::JpxColor::Enumerated(16) if n == 3 => Value::name(b"DeviceRGB"),
            jpx::JpxColor::Enumerated(17) if n == 1 => Value::name(b"DeviceGray"),
            _ => {
                return Err(Error::unsupported(
                    crate::features::JPX_COLOUR,
                    "JPX colour information is not representable",
                ));
            }
        });
    }
    image.components = n;
    image.bits_per_component = first.bits;
    let count = p::budget(image.width, image.height, n, first.bits, limits)?;
    let pixels = count / usize::from(n);
    if components.iter().any(|c| c.samples.len() != pixels) {
        return Err(Error::syntax("invalid JPX sample count"));
    }
    if first.bits <= 8 {
        let mut samples = Vec::with_capacity(count);
        for i in 0..pixels {
            for c in &components {
                samples.push(
                    u8::try_from(c.samples[i])
                        .map_err(|_| Error::syntax("JPX sample exceeds precision"))?,
                );
            }
        }
        image.samples = Samples::Bytes(samples);
    } else {
        let mut samples = Vec::with_capacity(count);
        for i in 0..pixels {
            for c in &components {
                samples.push(c.samples[i]);
            }
        }
        image.samples = Samples::Words(samples);
    }
    Ok(())
}
