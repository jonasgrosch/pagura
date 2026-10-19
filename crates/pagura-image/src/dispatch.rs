//! Image codec dispatch and cross-checking of dictionary and stream metadata.
use crate::{DecodedImage, ImageKind, MaskKind, Samples, dct, jbig2, parameters as p};
use pagura_codec::ImageCodec;
use pagura_core::{Diagnostics, Dictionary, Error, Result, Value};
use pagura_document::Document;

type Codec = (ImageCodec, Option<Dictionary>);

pub(crate) fn decode(
    document: &Document,
    dict: &Dictionary,
    data: &[u8],
    codec: Option<&Codec>,
    cells: u8,
    diagnostics: &mut Diagnostics,
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
        reduction: None,
    };
    let params = codec.and_then(|(_, params)| params.as_ref());
    match codec.map(|(codec, _)| codec) {
        None => {
            image.bits_per_component = bits;
            image.samples = p::unpack_with_diagnostics(
                data,
                width,
                height,
                components,
                bits,
                &limits,
                diagnostics,
            )?;
        }
        Some(ImageCodec::Dct) => {
            decode_dct(data, params, &mut image, bits, &limits, cells, diagnostics)?;
        }
        Some(ImageCodec::CcittFax) => {
            if bits != 1 || components != 1 {
                return Err(Error::syntax("CCITT requires one-bit components"));
            }
            image.samples =
                crate::fax_image::decode(data, params, width, height, &limits, diagnostics)?;
        }
        Some(ImageCodec::Jbig2) => {
            if bits != 1 || components != 1 {
                return Err(Error::syntax("JBIG2 requires one-bit components"));
            }
            image.samples = decode_jbig2(document, dict, data, params, width, height)?;
        }
        Some(ImageCodec::Jpx) => {
            crate::jpx_image::decode(data, dict, &mut image, &limits, diagnostics)?;
        }
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

fn color_transform(params: Option<&Dictionary>) -> Result<Option<i64>> {
    params
        .and_then(|dict| dict.get(b"ColorTransform"))
        .map(|v| {
            v.as_integer()
                .ok_or(Error::syntax("invalid DCT ColorTransform"))
        })
        .transpose()
}

/// An independently validated lossless image cannot be a JPEG with a damaged
/// SOI: all Flate framing, checksums, row tags and sample counts must agree.
fn decode_dct(
    data: &[u8],
    params: Option<&Dictionary>,
    image: &mut DecodedImage,
    bits: u8,
    limits: &pagura_core::Limits,
    cells: u8,
    diagnostics: &mut Diagnostics,
) -> Result<()> {
    let (width, height, components) = (image.width, image.height, image.components);
    image.bits_per_component = 8;
    let transform = color_transform(params)?;
    let decoded = match dct::decode_cells(data, transform, limits, diagnostics, cells) {
        Ok(decoded) => decoded,
        Err(error) => {
            if !data.starts_with(&[0xff, 0xd8])
                && bits == 8
                && let Some(samples) =
                    predicted_flate(data, params, (width, height, components), limits)
            {
                diagnostics.repair(
                    pagura_core::Diagnostic::new(
                        crate::tags::FILTER_MISMATCH,
                        "DCTDecode replaced by validated Flate image data (7.4.4, 7.4.8)",
                    )
                    .at(pagura_core::Position::Buffer(0)),
                )?;
                image.samples = Samples::Bytes(samples);
                return Ok(());
            }
            return Err(error);
        }
    };
    if decoded.width != dct::scaled_length(width, decoded.cells)
        || decoded.height != dct::scaled_length(height, decoded.cells)
        || decoded.components != components
        || bits != 8
    {
        return Err(Error::syntax(
            "JPEG metadata disagrees with image dictionary",
        ));
    }
    image.samples = Samples::Bytes(decoded.samples);
    if decoded.cells < 8 {
        image.width = decoded.width;
        image.height = decoded.height;
        image.reduction = Some(crate::Reduction {
            cells: decoded.cells,
            width,
            height,
        });
    }
    Ok(())
}

fn predicted_flate(
    data: &[u8],
    params: Option<&Dictionary>,
    dimensions: (u32, u32, u8),
    limits: &pagura_core::Limits,
) -> Option<Vec<u8>> {
    let (width, height, components) = dimensions;
    let predictor = pagura_codec::Predictor::from_params(params).ok()?;
    if predictor.kind == pagura_codec::PredictorKind::None
        || predictor.columns != width
        || predictor.colors != components
        || predictor.bits_per_component != 8
    {
        return None;
    }
    let expected = usize::try_from(
        u64::from(width)
            .checked_mul(u64::from(height))?
            .checked_mul(u64::from(components))?,
    )
    .ok()?;
    let mut strict = Diagnostics::new(pagura_core::Recovery::Strict);
    let samples = pagura_codec::Filter::Flate { predictor }
        .decode(data, limits.decoded_bytes, &mut strict)
        .ok()?;
    (samples.len() == expected).then_some(samples)
}
