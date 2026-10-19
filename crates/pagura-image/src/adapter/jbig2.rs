//! `JBIG2Decode` images (7.4.7): page 1 of the embedded stream, with the
//! `/JBIG2Globals` stream, inverted to PDF sample polarity.
use super::Pixels;
use crate::parameters;
use pagura_core::{Dictionary, Error, Result, Value};
use pagura_document::Document;
use pagura_imagecodec::jbig2;

pub(crate) fn decode(
    document: &Document,
    dict: &Dictionary,
    data: &[u8],
    params: Option<&Dictionary>,
    width: u32,
    height: u32,
) -> Result<Pixels> {
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
    let limits = document.limits();
    let decoded = jbig2::decode(data, globals.as_deref(), &limits)?;
    if decoded.width() != width || decoded.height() != height {
        return Err(Error::syntax(
            "JBIG2 dimensions disagree with image dictionary",
        ));
    }
    // JBIG2 black is 1; PDF image samples default to black 0 (7.4.7).
    let count = parameters::budget(width, height, 1, 1, &limits)?;
    if count
        .checked_add(decoded.data().len())
        .is_none_or(|v| v > limits.decoded_bytes)
    {
        return Err(Error::limit("JBIG2 adapter sample budget exceeded"));
    }
    let stride = usize::try_from(width.div_ceil(8))
        .map_err(|_| Error::limit("JBIG2 row exceeds the address space"))?;
    let columns = usize::try_from(width).map_err(|_| Error::limit("JBIG2 width overflow"))?;
    let mut samples = Vec::with_capacity(count);
    for row in decoded.data().chunks_exact(stride.max(1)) {
        samples.extend((0..columns).map(|x| {
            let bit = row.get(x / 8).map_or(0, |byte| (byte >> (7 - x % 8)) & 1);
            bit ^ 1
        }));
    }
    Ok(Pixels::bilevel(samples))
}
