//! `DCTDecode` images (7.4.8): the JPEG must agree with the dictionary.
use super::Pixels;
use crate::{Precision, Samples, parameters};
use pagura_codec::{Filter, Predictor, PredictorKind};
use pagura_core::{Diagnostic, Diagnostics, Dictionary, Error, Limits, Position, Recovery, Result};
use pagura_imagecodec::dct;

/// The image geometry a dictionary declares.
pub(crate) struct Declared {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) components: u8,
    pub(crate) bits: u8,
}

pub(crate) fn decode(
    data: &[u8],
    params: Option<&Dictionary>,
    declared: &Declared,
    limits: &Limits,
    diagnostics: &mut Diagnostics,
) -> Result<Pixels> {
    if declared.bits != 8 {
        return Err(Error::syntax(
            "JPEG metadata disagrees with image dictionary",
        ));
    }
    let transform = color_transform(params)?;
    let decoded = match dct::decode_with_diagnostics(data, transform, limits, diagnostics) {
        Ok(decoded) => decoded,
        Err(error) => {
            return mislabeled_flate(data, params, declared, limits, diagnostics)
                .unwrap_or(Err(error));
        }
    };
    if decoded.width != declared.width
        || decoded.height != declared.height
        || decoded.components != declared.components
    {
        return Err(Error::syntax(
            "JPEG metadata disagrees with image dictionary",
        ));
    }
    Ok(Pixels {
        components: decoded.components,
        precision: Precision::Uniform(8),
        samples: Samples::Bytes(decoded.samples),
    })
}

/// Data that is not a JPEG, which starts with SOI (T.81 B.2.1), but is
/// exactly the declared eight-bit samples as predicted Flate data with valid
/// framing, checksum and row tags, is decoded as such: a reported repair
/// (7.4.4, 7.4.8). `None` when the data is not that.
fn mislabeled_flate(
    data: &[u8],
    params: Option<&Dictionary>,
    declared: &Declared,
    limits: &Limits,
    diagnostics: &mut Diagnostics,
) -> Option<Result<Pixels>> {
    if data.starts_with(&[0xff, 0xd8]) || declared.bits != 8 {
        return None;
    }
    let predictor = Predictor::from_params(params).ok()?;
    if predictor.kind == PredictorKind::None
        || predictor.columns != declared.width
        || predictor.colors != declared.components
        || predictor.bits_per_component != 8
    {
        return None;
    }
    let count = parameters::budget(
        declared.width,
        declared.height,
        declared.components,
        8,
        limits,
    )
    .ok()?;
    let samples = Filter::Flate { predictor }
        .decode(
            data,
            limits.decoded_bytes,
            &mut Diagnostics::new(Recovery::Strict),
        )
        .ok()
        .filter(|samples| samples.len() == count)?;
    let report = Diagnostic::new(
        crate::repairs::FILTER_MISMATCH,
        "DCTDecode data decoded as the predicted Flate data it is (7.4.4, 7.4.8)",
    )
    .at(Position::Buffer(0));
    Some(diagnostics.repair(report).map(|()| Pixels {
        components: declared.components,
        precision: Precision::Uniform(8),
        samples: Samples::Bytes(samples),
    }))
}

/// `/ColorTransform` (Table 13).
pub(crate) fn color_transform(params: Option<&Dictionary>) -> Result<Option<i64>> {
    params
        .and_then(|dict| dict.get(b"ColorTransform"))
        .map(|v| {
            v.as_integer()
                .ok_or(Error::syntax("invalid DCT ColorTransform"))
        })
        .transpose()
}
