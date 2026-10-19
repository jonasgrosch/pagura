//! Fax row geometry: 7.4.6 Table 11 and 8.9.5.1 Table 87.
use super::Pixels;
use crate::Precision;
use crate::parameters::{self, boolean, integer};
use pagura_core::convert::u32_to_usize;
use pagura_core::{Diagnostic, Diagnostics, Dictionary, Error, Limits, Result};
use pagura_imagecodec::ccitt::{self, CcittParams, Encoding};
use std::num::NonZeroU32;

pub(crate) fn decode(
    data: &[u8],
    dictionary: Option<&Dictionary>,
    width: u32,
    height: u32,
    limits: &Limits,
    diagnostics: &mut Diagnostics,
) -> Result<Pixels> {
    let params = params(dictionary)?;
    if params.columns.get() != width {
        diagnostics.repair(Diagnostic::new(crate::repairs::CCITT_COLUMNS,
            "CCITT Columns retained for decoding; rows cropped or white-padded to image Width (7.4.6 Table 11; 8.9.5.1 Table 87)")
            )?;
        parameters::budget(params.columns.get(), height, 1, 1, limits)?;
    }
    let packed = ccitt::decode_with_diagnostics(data, &params, limits, diagnostics)?;
    if params.columns.get() == width {
        let samples =
            parameters::unpack_with_diagnostics(&packed, width, height, 1, 1, limits, diagnostics)?;
        return Ok(Pixels {
            components: 1,
            precision: Precision::Uniform(1),
            samples,
        });
    }
    let count = parameters::budget(width, height, 1, 1, limits)?;
    if packed
        .capacity()
        .checked_add(count)
        .is_none_or(|n| n > limits.decoded_bytes)
    {
        return Err(Error::limit("CCITT image adapter storage budget exceeded"));
    }
    let stride = usize::try_from(u64::from(params.columns.get()).div_ceil(8))
        .map_err(|_| Error::limit("CCITT row exceeds addressable samples"))?;
    let length = stride
        .checked_mul(u32_to_usize(height))
        .ok_or(Error::limit("CCITT image size overflow"))?;
    let data = parameters::packed_data(&packed, length, diagnostics)?;
    // Columns controls encoded runs and reference rows. Width controls the
    // painting grid. Cropping is on the right; extension uses fax white before
    // applying Decode. Byte-alignment bits never become image samples.
    let mut samples = vec![u8::from(!params.black_is_1); count];
    for (y, row) in samples.chunks_exact_mut(u32_to_usize(width)).enumerate() {
        for (x, sample) in row
            .iter_mut()
            .take(u32_to_usize(params.columns.get()))
            .enumerate()
        {
            let byte = data.get(y * stride + x / 8).copied().unwrap_or(0);
            *sample = (byte >> (7 - x % 8)) & 1;
        }
    }
    Ok(Pixels::bilevel(samples))
}

/// Reads the Table 11 parameters, including defaults.
pub(crate) fn params(dictionary: Option<&Dictionary>) -> Result<CcittParams> {
    let mut params = CcittParams::default();
    let Some(p) = dictionary else {
        return Ok(params);
    };
    params.encoding = Encoding::from_k(integer(p, b"K", 0)?);
    params.columns = u32::try_from(integer(p, b"Columns", 1728)?)
        .ok()
        .and_then(NonZeroU32::new)
        .ok_or(Error::syntax("invalid CCITT Columns"))?;
    params.rows = NonZeroU32::new(
        u32::try_from(integer(p, b"Rows", 0)?).map_err(|_| Error::syntax("invalid CCITT Rows"))?,
    );
    params.end_of_line = boolean(p, b"EndOfLine", false)?;
    params.encoded_byte_align = boolean(p, b"EncodedByteAlign", false)?;
    params.end_of_block = boolean(p, b"EndOfBlock", true)?;
    params.black_is_1 = boolean(p, b"BlackIs1", false)?;
    params.damaged_rows_before_error = u32::try_from(integer(p, b"DamagedRowsBeforeError", 0)?)
        .map_err(|_| Error::syntax("invalid CCITT damaged row count"))?;
    Ok(params)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pagura_core::{ErrorKind, Name, Recovery, Value};
    use pagura_document::{Document, OpenOptions};

    fn params(columns: i64) -> Dictionary {
        [
            (Name::new(b"Columns"), Value::Integer(columns)),
            (Name::new(b"K"), Value::Integer(-1)),
        ]
        .into_iter()
        .collect()
    }

    #[test]
    fn source_grid_budget_precedes_decoding_and_cropping() {
        let mut reports = Diagnostics::new(Recovery::Repair);
        let limits = Limits {
            pixels: 8,
            ..Limits::default()
        };
        let error = decode(&[], Some(&params(1024)), 1, 1, &limits, &mut reports).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit);
        assert_eq!(
            reports.as_slice()[0].repair(),
            Some(crate::repairs::CCITT_COLUMNS)
        );
    }

    #[test]
    fn packed_source_and_output_must_fit_together() {
        let document = Document::open(
            pagura_testkit::fixture("image", "fax-columns-5-width-7-g4-black0.pdf"),
            &OpenOptions::default(),
        )
        .unwrap();
        let stream = document
            .image_stream(document.object(pagura_core::ObjectId::new(3, 0)).unwrap())
            .unwrap();
        let limits = Limits {
            decoded_bytes: 1024 * 3,
            ..Limits::default()
        };
        let error = decode(
            &stream.data,
            Some(&params(5)),
            1024,
            3,
            &limits,
            &mut Diagnostics::new(Recovery::Repair),
        )
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit);
        assert_eq!(error.message, "CCITT image adapter storage budget exceeded");
    }

    #[test]
    fn source_errors_are_not_hidden_by_grid_cropping() {
        // A first V(+1) would put the changing element beyond Columns.
        let error = decode(
            &[0x60],
            Some(&params(5)),
            3,
            1,
            &Limits::default(),
            &mut Diagnostics::new(Recovery::Repair),
        )
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax);
        assert_eq!(error.repair, None);
    }
}
