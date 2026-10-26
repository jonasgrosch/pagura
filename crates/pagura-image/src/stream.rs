//! Narrow parameter recovery before image dispatch (7.3.8.1 Table 5, 7.4.1).
use pagura_core::{Diagnostic, Diagnostics, Error, ErrorKind, Position, Result, Value};
use pagura_document::{Document, ImageStream, IndirectObject};

pub(crate) fn read(
    document: &Document,
    image: &IndirectObject,
    diagnostics: &mut Diagnostics,
) -> Result<ImageStream> {
    let result = read_inner(document, image, diagnostics);
    if result
        .as_ref()
        .is_err_and(|e| e.kind == ErrorKind::Syntax && e.repair.is_none())
    {
        return predictor_precision(document, image, diagnostics).unwrap_or(result);
    }
    result
}

fn read_inner(
    document: &Document,
    image: &IndirectObject,
    diagnostics: &mut Diagnostics,
) -> Result<ImageStream> {
    let dictionary = image
        .dictionary()
        .ok_or(Error::syntax("image has no dictionary"))?;
    let size = expected_size(document, dictionary);
    let Some(params) = dictionary.get(b"DecodeParms") else {
        return document.image_stream_sized(image, None, size);
    };
    let params = document.resolve(params)?;
    let filter = dictionary
        .get(b"Filter")
        .map(|v| document.resolve(v))
        .transpose()?;
    let names = match filter {
        Some(value @ Value::Name(_)) => std::slice::from_ref(value),
        Some(Value::Array(names)) => names,
        _ => return document.image_stream_sized(image, dictionary.get(b"DecodeParms"), size),
    };
    let Some(params) = repaired_params(document, names, params)? else {
        return document.image_stream_sized(image, dictionary.get(b"DecodeParms"), size);
    };
    diagnostics.repair(
        Diagnostic::new(
            crate::tags::DECODE_PARAMS,
            "malformed image DecodeParms entry treated as absent (7.3.8.1 Table 5, 7.4.1, 7.4.9)",
        )
        .at(Position::Stream {
            object: image.id(),
            offset: 0,
        }),
    )?;
    document.image_stream_sized(image, Some(&params), size)
}

/// The sample bytes an image of a device colour space declares, as an
/// allocation hint for its decoded stream; 0 when they are not evident.
fn expected_size(document: &Document, dictionary: &pagura_core::Dictionary) -> usize {
    let value = |key: &[u8]| dictionary.get(key).and_then(|v| document.resolve(v).ok());
    let integer = |key: &[u8]| {
        value(key)
            .and_then(Value::as_integer)
            .and_then(|n| usize::try_from(n).ok())
    };
    let (Some(width), Some(height)) = (integer(b"Width"), integer(b"Height")) else {
        return 0;
    };
    let (components, bits) = if value(b"ImageMask").and_then(Value::as_bool) == Some(true) {
        (1, 1)
    } else {
        let components = match value(b"ColorSpace")
            .and_then(Value::as_name)
            .map(pagura_core::Name::as_bytes)
        {
            Some(b"DeviceGray" | b"G") => 1,
            Some(b"DeviceRGB" | b"RGB") => 3,
            Some(b"DeviceCMYK" | b"CMYK") => 4,
            _ => return 0,
        };
        (components, integer(b"BitsPerComponent").unwrap_or(8))
    };
    width
        .saturating_mul(components)
        .saturating_mul(bits)
        .div_ceil(8)
        .saturating_mul(height)
}

/// Preserve array alignment and all dictionaries. A string spelling `null`
/// has an unambiguous intended default; arbitrary wrong types are discarded
/// only for JPXDecode, whose interpretation has no external parameters (7.4.9).
fn repaired_params(document: &Document, names: &[Value], params: &Value) -> Result<Option<Value>> {
    let entries = match params {
        Value::Array(entries) if entries.len() == names.len() => entries.as_slice(),
        Value::Null | Value::Array(_) => return Ok(None),
        value if names.len() == 1 => std::slice::from_ref(value),
        _ => return Ok(None),
    };
    let mut repaired = false;
    let mut normalized = Vec::with_capacity(entries.len());
    for (name, value) in names.iter().zip(entries) {
        let name = document.resolve(name)?;
        let value = document.resolve(value)?;
        let invalid = !matches!(value, Value::Null | Value::Dictionary(_));
        let absent =
            value.as_string() == Some(b"null") || name.as_name().is_some_and(|n| n == b"JPXDecode");
        if invalid && absent {
            repaired = true;
            normalized.push(Value::Null);
        } else {
            normalized.push(value.clone());
        }
    }
    Ok(repaired.then(|| match params {
        Value::Array(_) => Value::Array(normalized),
        _ => normalized.pop().unwrap_or(Value::Null),
    }))
}

/// A syntax failure plus valid, conflicting predictor precision gives one
/// bounded retry. Successful unusual layouts keep their original interpretation;
/// failed retries preserve the original error and do not report a repair.
fn predictor_precision(
    document: &Document,
    image: &IndirectObject,
    diagnostics: &mut Diagnostics,
) -> Option<Result<ImageStream>> {
    let dictionary = image.dictionary()?;
    let bits = dictionary.get(b"BitsPerComponent")?.as_integer()?;
    if !matches!(bits, 1 | 2 | 4 | 8 | 16) {
        return None;
    }
    let params = document
        .resolve_dictionary(dictionary.get(b"DecodeParms")?)
        .ok()?;
    let predictor = pagura_codec::Predictor::from_params(Some(params)).ok()?;
    if i64::from(predictor.bits_per_component) == bits
        || predictor.kind == pagura_codec::PredictorKind::None
    {
        return None;
    }
    let mut params = params.clone();
    params.insert(
        pagura_core::Name::new(b"BitsPerComponent"),
        Value::Integer(bits),
    );
    let stream = document
        .image_stream_with_parameters(image, Some(&Value::Dictionary(params)))
        .ok()?;
    Some(
        diagnostics
            .repair(
                Diagnostic::new(
                    crate::tags::PREDICTOR_PRECISION,
                    "predictor data decoded at image BitsPerComponent (7.4.4.3)",
                )
                .at(Position::Stream {
                    object: image.id(),
                    offset: 0,
                }),
            )
            .map(|()| stream),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use pagura_core::{Limits, ObjectId, Recovery};
    use pagura_document::OpenOptions;
    use pagura_testkit::fixture;

    #[test]
    fn failed_predictor_retry_preserves_original_error_without_diagnostics() {
        for (name, decoded_bytes) in [
            (
                "predictor-retry-invalid-tag",
                Limits::default().decoded_bytes,
            ),
            ("predictor-image-precision", 1),
        ] {
            for recovery in [Recovery::Strict, Recovery::Repair] {
                let document = Document::open(
                    fixture("recovery", &format!("{name}.pdf")),
                    &OpenOptions {
                        recovery,
                        limits: Limits {
                            decoded_bytes,
                            ..Limits::default()
                        },
                        ..OpenOptions::default()
                    },
                )
                .unwrap();
                let image = document.object(ObjectId::new(5, 0)).unwrap();
                let original = document.image_stream(image).unwrap_err();
                let mut diagnostics = Diagnostics::new(recovery);
                let error = read(&document, image, &mut diagnostics).unwrap_err();
                assert_eq!(error, original, "{name}: {recovery:?}");
                assert!(diagnostics.as_slice().is_empty(), "{name}: {recovery:?}");
                assert!(document.diagnostics().is_empty(), "{name}: {recovery:?}");
            }
        }
    }
}
