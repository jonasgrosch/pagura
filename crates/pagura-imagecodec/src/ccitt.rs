//! CCITT Group 3 and Group 4 fax decoding (ITU-T T.4 and T.6) for
//! `CCITTFaxDecode` (7.4.6): row framing, end-of-line and end-of-block
//! codes and damaged-row recovery over the shared T.4/T.6 row decoder.
use crate::t4::{self, Bits, Color, EOL, EOL_BITS, Extensions};
use pagura_core::bytes::Cursor;
use pagura_core::{Diagnostic, Diagnostics, Error, ErrorKind, Limits, Meter, Recovery, Result};
use std::num::NonZeroU32;

/// How rows are coded: the meaning of `K` (Table 11).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Encoding {
    /// `K = 0`: Group 3, every row one-dimensional (T.4 4.1).
    #[default]
    Group3OneD,
    /// `K > 0`: Group 3, mixed: a tag bit after each EOL says whether the
    /// row is one- or two-dimensional (T.4 4.2). The magnitude of `K` bounds
    /// the encoder's run of two-dimensional rows and does not change
    /// decoding.
    Group3Mixed,
    /// `K < 0`: Group 4, every row two-dimensional (T.6).
    Group4,
}

impl Encoding {
    /// The encoding a `K` value selects.
    pub const fn from_k(k: i64) -> Self {
        match k {
            0 => Self::Group3OneD,
            1.. => Self::Group3Mixed,
            _ => Self::Group4,
        }
    }
}

/// `CCITTFaxDecode` parameters (Table 11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent boolean entries of Table 11"
)]
pub struct CcittParams {
    /// `K`: how rows are coded.
    pub encoding: Encoding,
    /// `EndOfLine`: every row starts with an end-of-line code.
    pub end_of_line: bool,
    /// `EncodedByteAlign`: every row starts on a byte boundary.
    pub encoded_byte_align: bool,
    /// `Columns`: pixels per row (default 1728).
    pub columns: NonZeroU32,
    /// `Rows`: declared rows, absent when zero or unknown.
    pub rows: Option<NonZeroU32>,
    /// `EndOfBlock`: data ends with an end-of-block code, which takes
    /// precedence over `rows` (default true).
    pub end_of_block: bool,
    /// `BlackIs1`: black pixels are one bits.
    pub black_is_1: bool,
    /// `DamagedRowsBeforeError`: damaged rows tolerated when `end_of_line`
    /// is true and the encoding is Group 3.
    pub damaged_rows_before_error: u32,
}

impl Default for CcittParams {
    fn default() -> Self {
        Self {
            encoding: Encoding::Group3OneD,
            end_of_line: false,
            encoded_byte_align: false,
            columns: NonZeroU32::new(1728).unwrap_or(NonZeroU32::MIN),
            rows: None,
            end_of_block: true,
            black_is_1: false,
            damaged_rows_before_error: 0,
        }
    }
}

/// Decodes packed one-bit rows, most significant bit first. Zero denotes
/// black unless `black_is_1` is set. Unused low bits of each row are zero.
/// Decoding is strict; use [`decode_with_diagnostics`] to request reported
/// row recovery.
///
/// # Errors
/// Returns positioned syntax and limit errors, or `Unsupported` for the
/// uncompressed extension mode.
pub fn decode(data: &[u8], params: &CcittParams, limits: &Limits) -> Result<Vec<u8>> {
    decode_with_diagnostics(
        data,
        params,
        limits,
        &mut Diagnostics::new(Recovery::Strict),
    )
}

/// Decodes fax data with Table 11 damaged-row substitution reported through
/// `diagnostics`. The tolerance counts damaged rows across the image and
/// applies only to Group 3 with `end_of_line` true. The next EOL must be
/// present; truncated rows without a following EOL cannot be recovered.
///
/// Work is bounded by `decode_work` (one unit per bit read), the row lists
/// by `decode_bytes`, the output by `pixels` and `decoded_bytes`.
///
/// # Errors
/// Returns positioned errors for invalid input, exhausted budgets, strict
/// rejection of a repair, or damage beyond the declared tolerance.
pub fn decode_with_diagnostics(
    data: &[u8],
    params: &CcittParams,
    limits: &Limits,
    diagnostics: &mut Diagnostics,
) -> Result<Vec<u8>> {
    crate::sample_count(params.columns.get(), 1, 1, 1, limits)?;
    if let Some(rows) = params.rows.filter(|_| !params.end_of_block) {
        crate::sample_count(params.columns.get(), rows.get(), 1, 1, limits)?;
    }
    let width = usize::try_from(params.columns.get())
        .map_err(|_| Error::limit("CCITT row exceeds the address space"))?;
    let stride = width.div_ceil(8);
    let mut memory = Meter::new(limits.decode_bytes, "CCITT row workspace budget exceeded");
    // Two changing-element lists keep each row's work linear in its changes.
    let (mut reference, mut changes) = (Vec::new(), Vec::new());
    for list in [&mut reference, &mut changes] {
        memory.reserve(list, t4::max_changes(width).saturating_add(1))?;
    }
    let mut work = Meter::new(limits.decode_work, "CCITT work budget exceeded");
    let mut bits = Bits::new(
        Cursor::new(data).with_message("truncated CCITT data"),
        &mut work,
    );
    let mut output = Vec::new();
    let mut rows = 0usize;
    let mut damage = Damage::default();
    let ones = if params.black_is_1 {
        Color::Black
    } else {
        Color::White
    };
    loop {
        if !params.end_of_block
            && params
                .rows
                .is_some_and(|n| rows >= pagura_core::convert::u32_to_usize(n.get()))
        {
            break;
        }
        let Some(one_d) = next_row(&mut bits, params, rows == 0, diagnostics)? else {
            break;
        };
        let next_rows = rows
            .checked_add(1)
            .filter(|&v| v <= limits.pixels / width)
            .ok_or(Error::limit("CCITT pixel budget exceeded"))?;
        let length = output
            .len()
            .checked_add(stride)
            .filter(|&v| v <= limits.decoded_bytes)
            .ok_or(Error::limit("CCITT output budget exceeded"))?;
        changes.clear();
        let row_start = bits.mark();
        let row = if one_d {
            t4::row_1d(&mut bits, width, &mut changes)
        } else {
            t4::row_2d(
                &mut bits,
                width,
                &reference,
                &mut changes,
                Extensions::Unsupported,
            )
        };
        if let Err(error) = row {
            if !damage.recoverable(&error, params) {
                return Err(error);
            }
            bits.reset(row_start);
            resynchronize(&mut bits)?;
            changes.clear();
            if !damage.previous {
                changes.extend_from_slice(&reference);
            }
            damage.report(&error, diagnostics)?;
        } else {
            damage.previous = false;
        }
        let start = output.len();
        output.resize(length, 0);
        t4::pack_row(&mut output[start..], &changes, width, ones);
        std::mem::swap(&mut reference, &mut changes);
        rows = next_rows;
    }
    Ok(output)
}

/// Damaged rows so far (Table 11, `DamagedRowsBeforeError`).
#[derive(Default)]
struct Damage {
    count: u32,
    /// Whether the previous row was damaged; a damaged row after it is
    /// replaced by white instead of the previous row.
    previous: bool,
}

impl Damage {
    /// Whether a row that failed with `error` can be recovered: a syntax
    /// error in Group 3 data with EOLs, with tolerance left.
    fn recoverable(&self, error: &Error, params: &CcittParams) -> bool {
        error.kind == ErrorKind::Syntax
            && params.encoding != Encoding::Group4
            && params.end_of_line
            && self.count < params.damaged_rows_before_error
    }

    /// Reports a recovered row.
    fn report(&mut self, error: &Error, diagnostics: &mut Diagnostics) -> Result<()> {
        diagnostics.repair(
            Diagnostic::new(
                crate::repairs::CCITT_DAMAGED_ROW,
                "damaged fax row replaced after EOL resynchronization",
            )
            .at(error.position),
        )?;
        self.count += 1;
        self.previous = true;
        Ok(())
    }
}

/// Reads what precedes the next row: byte alignment, an end-of-block code,
/// an EOL and the one- or two-dimensional tag bit. Returns whether the row
/// is one-dimensional, or None at the end of the data.
fn next_row(
    bits: &mut Bits<'_, '_>,
    params: &CcittParams,
    first: bool,
    diagnostics: &mut Diagnostics,
) -> Result<Option<bool>> {
    if params.encoded_byte_align && bits.align()?.value() != 0 {
        return Err(bits.error("nonzero CCITT alignment padding"));
    }
    if block_end(bits, params.encoding)?
        || (params.encoding == Encoding::Group4 && padded_block_end(bits, diagnostics)?)
    {
        return Ok(None);
    }
    if bits.only_fill_remains() {
        if params.end_of_block {
            return Err(bits.error("missing CCITT end of block"));
        }
        return Ok(None);
    }
    if bits.zeros_before_one()?.is_some_and(|zeros| zeros >= 11)
        || params.end_of_line
        || params.encoding == Encoding::Group3Mixed
    {
        eol(bits)?;
    }
    let one_d = match params.encoding {
        Encoding::Group3OneD => true,
        Encoding::Group3Mixed => bits.bit()?,
        Encoding::Group4 => false,
    };
    if first && params.encoding == Encoding::Group3Mixed && !one_d {
        return Err(bits.error("first mixed CCITT row must be one-dimensional"));
    }
    Ok(Some(one_d))
}

/// Reads an end-of-line code, allowing fill zeros before it (T.4 4.1.2).
fn eol(bits: &mut Bits<'_, '_>) -> Result<()> {
    let at = bits.position();
    let mut zeros = 0;
    while !bits.bit()? {
        zeros += 1;
    }
    if zeros < 11 {
        return Err(Error::syntax("invalid CCITT end of line").at(at));
    }
    Ok(())
}

/// Moves to the start of the next end-of-line code after a damaged row: the
/// last eleven zeros before a one bit.
fn resynchronize(bits: &mut Bits<'_, '_>) -> Result<()> {
    let mut zeros = 0;
    let mut run_start = bits.mark();
    loop {
        let here = bits.mark();
        if !bits.bit()? {
            if zeros == 0 {
                run_start = here;
            }
            zeros += 1;
        } else if zeros >= 11 {
            bits.reset(run_start);
            // Fill zeros before the code stay consumed.
            for _ in 11..zeros {
                bits.bit()?;
            }
            return Ok(());
        } else {
            zeros = 0;
        }
    }
}

/// Consumes an end-of-block code if one comes next: EOFB, two EOLs, for
/// Group 4 (T.6 2.4.1.1), or RTC, six EOLs, for Group 3 (T.4 4.1.4), each
/// followed by a tag bit in mixed coding.
fn block_end(bits: &mut Bits<'_, '_>, encoding: Encoding) -> Result<bool> {
    let start = bits.mark();
    let count = if encoding == Encoding::Group4 { 2 } else { 6 };
    for _ in 0..count {
        if !bits.peek(EOL_BITS, EOL) {
            bits.reset(start);
            return Ok(false);
        }
        bits.bits(EOL_BITS)?;
        if encoding == Encoding::Group3Mixed && bits.peek(1, 1) {
            bits.bit()?;
        }
    }
    Ok(true)
}

/// Consumes zero fill up to a byte boundary followed by EOFB, reporting the
/// fill as a repair.
fn padded_block_end(bits: &mut Bits<'_, '_>, diagnostics: &mut Diagnostics) -> Result<bool> {
    let fill = bits.unread_in_byte();
    let Ok(width) = u32::try_from(fill) else {
        return Ok(false);
    };
    if fill == 0 || !bits.peek(width, 0) {
        return Ok(false);
    }
    let start = bits.mark();
    let at = bits.position();
    bits.bits(width)?;
    if !block_end(bits, Encoding::Group4)? {
        bits.reset(start);
        return Ok(false);
    }
    diagnostics.repair(
        Diagnostic::new(
            crate::repairs::CCITT_EOB_PADDING,
            "zero fill before CCITT EOFB ignored (T.6 2.4.1.1-2; Table 11)",
        )
        .at(at),
    )?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::{CcittParams, Encoding, decode};
    use pagura_core::{ErrorKind, Limits};
    use std::num::NonZeroU32;

    #[test]
    fn k_selects_the_encoding() {
        assert_eq!(Encoding::from_k(0), Encoding::Group3OneD);
        assert_eq!(Encoding::from_k(4), Encoding::Group3Mixed);
        assert_eq!(Encoding::from_k(-1), Encoding::Group4);
        assert_eq!(CcittParams::default().columns.get(), 1728);
    }

    #[test]
    fn work_is_charged_per_bit_against_decode_work() {
        let params = CcittParams {
            encoding: Encoding::Group4,
            columns: NonZeroU32::new(8).unwrap(),
            ..CcittParams::default()
        };
        // V0 against an empty reference ends the row at once; then EOFB.
        let data = [0b1000_0000, 0b0000_1000, 0b0000_0000, 0b1000_0000];
        assert_eq!(decode(&data, &params, &Limits::default()).unwrap(), [0xff]);
        let limits = Limits {
            decode_work: 0,
            ..Limits::default()
        };
        let error = decode(&data, &params, &limits).unwrap_err();
        assert_eq!(
            (error.kind, error.message),
            (ErrorKind::Limit, "CCITT work budget exceeded")
        );
    }
}
