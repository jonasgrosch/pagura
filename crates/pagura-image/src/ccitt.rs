//! CCITT Group 3 and Group 4 fax decoding (ITU-T T.4, T.6) for
//! `CCITTFaxDecode` (clause 7.4.6).

use pagura_core::{Dictionary, Error, Feature, Limits, Result};

/// Placeholder tag while the decoder is under construction.
const PENDING: Feature = Feature::new("filter.CCITTFaxDecode");

/// `CCITTFaxDecode` parameters (Table 11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors the independent boolean entries of Table 11"
)]
pub struct CcittParams {
    /// `/K`: below 0 pure 2-D (G4), 0 pure 1-D (G3), above 0 mixed.
    pub k: i64,
    /// `/EndOfLine`.
    pub end_of_line: bool,
    /// `/EncodedByteAlign`.
    pub encoded_byte_align: bool,
    /// `/Columns`, default 1728.
    pub columns: u32,
    /// `/Rows`, if given.
    pub rows: Option<u32>,
    /// `/EndOfBlock`, default true.
    pub end_of_block: bool,
    /// `/BlackIs1`.
    pub black_is_1: bool,
    /// `/DamagedRowsBeforeError`.
    pub damaged_rows_before_error: u32,
}

impl CcittParams {
    /// Reads the parameters from `/DecodeParms`, applying Table 11 defaults.
    ///
    /// # Errors
    ///
    /// Returns a syntax error for values of the wrong type or range.
    pub fn from_dictionary(params: Option<&Dictionary>) -> Result<Self> {
        let _ = params;
        Err(Error::unsupported(
            PENDING,
            "CCITT parameters are not implemented yet",
        ))
    }
}

/// Decodes fax data to 1-bit rows, MSB first, each padded to a byte, with
/// 0 = black unless `black_is_1`.
///
/// # Errors
///
/// Returns syntax errors for corrupt data (subject to
/// `damaged_rows_before_error`) and limit errors beyond `limits`.
pub fn decode(data: &[u8], params: &CcittParams, limits: &Limits) -> Result<Vec<u8>> {
    let _ = (data, params, limits);
    Err(Error::unsupported(
        PENDING,
        "CCITT decoding is not implemented yet",
    ))
}
