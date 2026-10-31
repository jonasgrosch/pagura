//! Image codec dispatch and cross-checking of dictionary and stream metadata.
use crate::{DecodedImage, Samples, SharedBytes, dct, jbig2, parameters as p};
use pagura_core::{Diagnostics, Dictionary, Error, Result, Value};
use pagura_document::Document;

/// Samples of unfiltered image data. Whole bytes of exactly the declared
/// extent are the samples themselves, shared with `data` under the budget
/// unpacking them would meet; other data are unpacked into samples of their
/// own.
pub(crate) fn unfiltered(
    data: &SharedBytes,
    (width, height, components, bits): (u32, u32, u8, u8),
    limits: &pagura_core::Limits,
    diagnostics: &mut Diagnostics,
) -> Result<Samples> {
    let count = usize::try_from(u64::from(width) * u64::from(height) * u64::from(components)).ok();
    if bits == 8 && count == Some(data.len()) {
        p::budget(width, height, components, bits, limits)?;
        return Ok(Samples::Bytes(data.clone()));
    }
    p::unpack_with_diagnostics(data, width, height, components, bits, limits, diagnostics)
}

/// The data of the `JBIG2Globals` stream of an image whose JBIG2 filter has
/// `params` (7.4.7).
pub(crate) fn jbig2_globals<'a>(
    document: &'a Document,
    dict: &Dictionary,
    params: Option<&Dictionary>,
) -> Result<Option<std::borrow::Cow<'a, [u8]>>> {
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
    original
        .and_then(Value::as_dictionary)
        .or(params)
        .and_then(|p| p.get(b"JBIG2Globals"))
        .map(|v| {
            let id = v
                .as_reference()
                .ok_or(Error::syntax("JBIG2Globals must reference a stream"))?;
            document.stream_data(document.object(id)?)
        })
        .transpose()
}

/// The samples of a JBIG2 image of `width` × `height` with `globals`.
pub(crate) fn jbig2_samples(
    data: &[u8],
    globals: Option<&[u8]>,
    (width, height): (u32, u32),
    limits: &pagura_core::Limits,
) -> Result<Samples> {
    let decoded = jbig2::decode(data, globals, limits)?;
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
    let count = p::budget(width, height, 1, 1, limits)?;
    if count
        .checked_add(inverted.len())
        .is_none_or(|v| v > limits.decoded_bytes)
    {
        return Err(Error::limit("JBIG2 adapter sample budget exceeded"));
    }
    p::unpack(&inverted, width, height, 1, 1, limits)
}

pub(crate) fn color_transform(params: Option<&Dictionary>) -> Result<Option<i64>> {
    params
        .and_then(|dict| dict.get(b"ColorTransform"))
        .map(|v| {
            v.as_integer()
                .ok_or(Error::syntax("invalid DCT ColorTransform"))
        })
        .transpose()
}

/// How DCTDecode samples are decoded.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DctOptions {
    /// Whether native planes are kept where the frame allows.
    pub(crate) deferred: bool,
    /// Whether native planes store rows and columns exchanged.
    pub(crate) transposed: bool,
}

/// Decodes DCTDecode samples into `image` as `options` ask. Returns whether
/// the samples hold rows and columns exchanged: native planes asked for
/// that way; other samples never are.
///
/// An independently validated lossless image cannot be a JPEG with a damaged
/// SOI: all Flate framing, checksums, row tags and sample counts must agree.
pub(crate) fn decode_dct(
    data: &[u8],
    params: Option<&Dictionary>,
    (image, bits): (&mut DecodedImage, u8),
    options: DctOptions,
    limits: &pagura_core::Limits,
    diagnostics: &mut Diagnostics,
) -> Result<bool> {
    let DctOptions {
        deferred,
        transposed,
    } = options;
    let (width, height, components) = (image.width, image.height, image.components);
    image.bits_per_component = 8;
    let transform = color_transform(params)?;
    // A deferred decode that fails or does not apply leaves no trace: the
    // ordinary decoder below then reports its own errors and repairs.
    if deferred
        && bits == 8
        && let Some(decoded) = dct::decode_deferred(
            data,
            transform,
            limits,
            dct::Native { transposed },
            diagnostics,
        )
    {
        native_samples(image, decoded, (components, transposed))?;
        return Ok(transposed);
    }
    let decoded = match dct::decode_with_diagnostics(data, transform, limits, diagnostics) {
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
                image.samples = Samples::Bytes(samples.into());
                return Ok(false);
            }
            return Err(error);
        }
    };
    if decoded.width != width
        || decoded.height != height
        || decoded.components != components
        || bits != 8
    {
        return Err(Error::syntax(
            "JPEG metadata disagrees with image dictionary",
        ));
    }
    image.samples = Samples::Bytes(decoded.samples.into());
    Ok(false)
}

/// Places a deferred decode's samples in `image`, checking them against
/// the dictionary's dimensions and components, exchanged when `transposed`.
fn native_samples(
    image: &mut DecodedImage,
    decoded: dct::Deferred,
    (components, transposed): (u8, bool),
) -> Result<()> {
    let (size, samples) = match decoded {
        dct::Deferred::Planar(planar) => (
            (planar.width(), planar.height(), 3),
            Samples::Planar(planar),
        ),
        dct::Deferred::Gray(image) => (
            (
                image.width as usize,
                image.height as usize,
                image.components,
            ),
            Samples::Bytes(image.samples.into()),
        ),
    };
    let declared = if transposed {
        (image.height, image.width)
    } else {
        (image.width, image.height)
    };
    if size != (declared.0 as usize, declared.1 as usize, components) {
        return Err(Error::syntax(
            "JPEG metadata disagrees with image dictionary",
        ));
    }
    image.samples = samples;
    (image.width, image.height) = declared;
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
