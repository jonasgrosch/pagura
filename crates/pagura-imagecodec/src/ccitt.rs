//! CCITT Group 3/4 fax (ITU-T T.4/T.6; ISO 32000-2 clause 7.4.6).
mod codes;
use pagura_core::{
    Diagnostic, Diagnostics, Error, ErrorKind, Limits, Position, Recovery, Repair, Result,
};

/// `CCITTFaxDecode` parameters (Table 11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent boolean entries of Table 11"
)]
pub struct CcittParams {
    /// Negative: Group 4; zero: Group 3 1-D; positive: Group 3 mixed.
    pub k: i64,
    /// Require end-of-line bit patterns.
    pub end_of_line: bool,
    /// Start encoded rows on byte boundaries.
    pub encoded_byte_align: bool,
    /// Pixels per row (default 1728).
    pub columns: u32,
    /// Declared rows; zero or absent means unknown.
    pub rows: Option<u32>,
    /// Terminate at EOFB/RTC, overriding Rows (default true).
    pub end_of_block: bool,
    /// Output black pixels as one bits.
    pub black_is_1: bool,
    /// Tolerated damaged rows when EndOfLine is true and K is nonnegative.
    pub damaged_rows_before_error: u32,
}
impl Default for CcittParams {
    fn default() -> Self {
        Self {
            k: 0,
            end_of_line: false,
            encoded_byte_align: false,
            columns: 1728,
            rows: None,
            end_of_block: true,
            black_is_1: false,
            damaged_rows_before_error: 0,
        }
    }
}
struct Bits<'a> {
    data: &'a [u8],
    at: usize,
    work: usize,
    cap: usize,
}
impl Bits<'_> {
    fn error(&self, message: &'static str) -> Error {
        Error::syntax(message).at(Position::Buffer(self.at / 8))
    }
    /// The next `width` bits (at most 17), MSB first, without consuming them.
    /// `None` unless all of them are present.
    fn peek(&self, width: usize) -> Option<usize> {
        if self.data.len().saturating_mul(8).saturating_sub(self.at) < width {
            return None;
        }
        let offset = self.at / 8;
        let byte = |i| usize::from(self.data.get(offset + i).copied().unwrap_or(0));
        let word = (byte(0) << 16) | (byte(1) << 8) | byte(2);
        Some((word >> (24 - self.at % 8 - width)) & ((1 << width) - 1))
    }
    /// Consumes `count` peeked bits and charges them as `count` calls to
    /// [`Self::bit`] would. Returns false, consuming nothing, when that would
    /// exceed the work budget; the bitwise path then reports the exact error.
    fn advance(&mut self, count: usize) -> bool {
        match self
            .work
            .checked_add(count)
            .filter(|&work| work <= self.cap)
        {
            Some(work) => {
                self.work = work;
                self.at += count;
                true
            }
            None => false,
        }
    }
    fn bit(&mut self) -> Result<u8> {
        self.work += 1;
        if self.work > self.cap {
            return Err(
                Error::limit("CCITT work budget exceeded").at(Position::Buffer(self.at / 8))
            );
        }
        let byte = self
            .data
            .get(self.at / 8)
            .ok_or(self.error("truncated CCITT data"))?;
        let bit = (byte >> (7 - self.at % 8)) & 1;
        self.at += 1;
        Ok(bit)
    }
    fn matches(&self, code: &str) -> bool {
        code.bytes().enumerate().all(|(i, c)| {
            self.data
                .get((self.at + i) / 8)
                .is_some_and(|b| (b >> (7 - (self.at + i) % 8)) & 1 == c - b'0')
        })
    }
    fn has_eol(&mut self) -> Result<bool> {
        let mut at = self.at;
        while self
            .data
            .get(at / 8)
            .is_some_and(|b| (b >> (7 - at % 8)) & 1 == 0)
        {
            self.work = self
                .work
                .checked_add(1)
                .filter(|&work| work <= self.cap)
                .ok_or(Error::limit("CCITT work budget exceeded").at(Position::Buffer(at / 8)))?;
            at += 1;
        }
        Ok(at - self.at >= 11 && at / 8 < self.data.len())
    }
    fn exhausted(&self) -> bool {
        let remaining = self.data.len().saturating_mul(8).saturating_sub(self.at);
        remaining < 8
            && (0..remaining)
                .all(|i| self.data[(self.at + i) / 8] & (128 >> ((self.at + i) % 8)) == 0)
    }
    fn eol(&mut self) -> Result<()> {
        let mut zeros = 0;
        while self.bit()? == 0 {
            zeros += 1;
        }
        if zeros < 11 {
            return Err(self.error("invalid CCITT end of line"));
        }
        Ok(())
    }
    fn align(&mut self) -> Result<()> {
        while !self.at.is_multiple_of(8) {
            if self.bit()? != 0 {
                return Err(self.error("nonzero CCITT alignment padding"));
            }
        }
        Ok(())
    }
    fn run(&mut self, black: bool, remaining: usize) -> Result<usize> {
        let mut total = 0usize;
        loop {
            let value = codes::run(self, black)?;
            total = total
                .checked_add(value)
                .filter(|&v| v <= remaining)
                .ok_or(self.error("CCITT run exceeds row"))?;
            if value < 64 {
                return Ok(total);
            }
        }
    }
}

/// Stable repair tags for fax data (ADR 0003).
pub mod tags {
    use super::Repair;
    /// Substitute the previous good row, or white after a damaged row,
    /// after locating the next EOL (ISO 32000-2 Table 11).
    pub const DAMAGED_ROW: Repair = Repair::new("ccitt.damaged-row");
    /// Accept up to seven zero fill bits before a byte-aligned Group 4 EOFB.
    /// T.6 2.4.1.1-2 and ISO Table 11 permit padding after the EOFB instead.
    pub const EOB_PADDING: Repair = Repair::new("ccitt.eob-padding");
}

/// Decodes packed one-bit rows, MSB first. Zero denotes black unless
/// `black_is_1` is set. Unused low bits of each row are zero. Decoding is
/// strict; use [`decode_with_diagnostics`] to request reported row recovery.
///
/// # Errors
/// Returns positioned syntax/limit errors or Unsupported extension modes.
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
/// applies only to nonnegative K with EndOfLine true. The next EOL must be
/// present; truncated rows without a following EOL cannot be recovered.
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
    decode_inner(data, params, limits, diagnostics).map_err(|e| e.at(Position::Buffer(0)))
}

fn decode_inner(
    data: &[u8],
    params: &CcittParams,
    limits: &Limits,
    diagnostics: &mut Diagnostics,
) -> Result<Vec<u8>> {
    if params.columns == 0 {
        return Err(Error::syntax("invalid CCITT Columns"));
    }
    crate::sample_count(params.columns, 1, 1, 1, limits)?;
    if data.len() > limits.stream_bytes || data.len().checked_mul(8).is_none() {
        return Err(Error::limit("CCITT input budget exceeded").at(Position::Buffer(0)));
    }
    if let Some(rows) = params.rows.filter(|_| !params.end_of_block) {
        crate::sample_count(params.columns, rows, 1, 1, limits)?;
    }
    let width = params.columns as usize;
    let stride = width.div_ceil(8);
    let scratch = workspace(width, stride, limits)?;
    let mut bits = Bits {
        data,
        at: 0,
        work: 0,
        cap: limits.operations,
    };
    let mut output = Vec::new();
    let (mut reference, mut changes) =
        (Vec::with_capacity(width + 2), Vec::with_capacity(width + 2));
    let mut rows = 0usize;
    let (mut damaged, mut previous_damaged) = (0u32, false);
    loop {
        if !params.end_of_block && params.rows.is_some_and(|n| rows >= n as usize) {
            break;
        }
        if params.encoded_byte_align {
            bits.align()?;
        }
        if block_end(&mut bits, params.k)
            || (params.k < 0 && padded_block_end(&mut bits, diagnostics)?)
        {
            break;
        }
        if bits.exhausted() {
            if params.end_of_block {
                return Err(bits.error("missing CCITT end of block"));
            }
            break;
        }
        if bits.has_eol()? || params.end_of_line || params.k > 0 {
            bits.eol()?;
        }
        let one_d = params.k == 0 || (params.k > 0 && bits.bit()? != 0);
        if rows == 0 && params.k > 0 && !one_d {
            return Err(bits.error("first mixed CCITT row must be one-dimensional"));
        }
        let next_rows = rows
            .checked_add(1)
            .filter(|&v| v <= limits.pixels / width)
            .ok_or(Error::limit("CCITT pixel budget exceeded"))?;
        let length = output
            .len()
            .checked_add(stride)
            .filter(|&v| v <= limits.decoded_bytes.saturating_sub(scratch))
            .ok_or(Error::limit("CCITT output budget exceeded"))?;
        changes.clear();
        let row_start = bits.at;
        let row_result = if one_d {
            row_1d(&mut bits, width, &mut changes)
        } else {
            row_2d(&mut bits, width, &reference, &mut changes)
        };
        let row_damaged = if let Err(error) = row_result {
            if error.kind != ErrorKind::Syntax
                || params.k < 0
                || !params.end_of_line
                || damaged >= params.damaged_rows_before_error
            {
                return Err(error);
            }
            bits.at = row_start;
            resynchronize(&mut bits)?;
            diagnostics.repair(
                Diagnostic::new(
                    tags::DAMAGED_ROW,
                    "damaged fax row replaced after EOL resynchronization",
                )
                .at(error.position),
            )?;
            changes.clear();
            if !previous_damaged {
                changes.extend_from_slice(&reference);
            }
            damaged += 1;
            true
        } else {
            false
        };
        let start = output.len();
        output.resize(length, 0);
        let row = &mut output[start..];
        pack_row(row, &changes, width, params.black_is_1);
        std::mem::swap(&mut reference, &mut changes);
        rows = next_rows;
        previous_damaged = row_damaged;
    }
    Ok(output)
}

fn workspace(width: usize, stride: usize, limits: &Limits) -> Result<usize> {
    // Two transition lists bound reference lookup and preserve linear row work.
    width
        .checked_add(2)
        .and_then(|v| v.checked_mul(2 * size_of::<usize>()))
        .and_then(|v| v.checked_add(stride))
        .filter(|&v| v <= limits.decoded_bytes)
        .ok_or(Error::limit("CCITT row workspace budget exceeded"))
}

/// Sets the one bits of each run `from..to` a byte range at a time. Changing
/// elements are nondecreasing and at most `width`.
fn pack_row(row: &mut [u8], changes: &[usize], width: usize, black_is_1: bool) {
    let (mut black, mut from) = (false, 0);
    for to in changes.iter().copied().chain(std::iter::once(width)) {
        if black == black_is_1 && to > from {
            let (first, last) = (from / 8, (to - 1) / 8);
            let left = 255_u8 >> (from % 8);
            let right = 255_u8 << (7 - (to - 1) % 8);
            if first == last {
                row[first] |= left & right;
            } else {
                row[first] |= left;
                row[first + 1..last].fill(255);
                row[last] |= right;
            }
        }
        from = to;
        black = !black;
    }
}

fn resynchronize(bits: &mut Bits<'_>) -> Result<()> {
    let mut zeros = 0;
    loop {
        if bits.bit()? == 0 {
            zeros += 1;
        } else if zeros >= 11 {
            bits.at -= 12;
            return Ok(());
        } else {
            zeros = 0;
        }
    }
}

fn block_end(bits: &mut Bits<'_>, k: i64) -> bool {
    let start = bits.at;
    let count = if k < 0 { 2 } else { 6 };
    for _ in 0..count {
        if !bits.matches("000000000001") {
            bits.at = start;
            return false;
        }
        bits.at += 12;
        if k > 0 && bits.matches("1") {
            bits.at += 1;
        }
    }
    true
}

fn padded_block_end(bits: &mut Bits<'_>, diagnostics: &mut Diagnostics) -> Result<bool> {
    let start = bits.at;
    let padding = 8 - start % 8;
    if padding == 8 || !bits.matches(&"0000000"[..padding]) {
        return Ok(false);
    }
    bits.at += padding;
    if !block_end(bits, -1) {
        bits.at = start;
        return Ok(false);
    }
    diagnostics.repair(
        Diagnostic::new(
            tags::EOB_PADDING,
            "zero fill before CCITT EOFB ignored (T.6 2.4.1.1-2; Table 11)",
        )
        .at(Position::Buffer(start / 8)),
    )?;
    Ok(true)
}

fn row_1d(bits: &mut Bits<'_>, width: usize, changes: &mut Vec<usize>) -> Result<()> {
    let (mut at, mut black) = (0, false);
    while at < width {
        let run = bits.run(black, width - at)?;
        at += run;
        changes.push(at);
        black = !black;
        if changes.len() > width + 1 {
            return Err(bits.error("too many CCITT zero-length runs"));
        }
    }
    Ok(())
}

fn row_2d(
    bits: &mut Bits<'_>,
    width: usize,
    reference: &[usize],
    changes: &mut Vec<usize>,
) -> Result<()> {
    let (mut at, mut black, mut cursor, mut first) = (0, false, 0, true);
    while at < width {
        while cursor < reference.len()
            && (reference[cursor] < at || (!first && reference[cursor] == at))
        {
            cursor += 1;
        }
        let index = if cursor % 2 == usize::from(black) {
            cursor
        } else {
            cursor + 1
        };
        let b1 = reference.get(index).copied().unwrap_or(width);
        let b2 = reference.get(index + 1).copied().unwrap_or(width);
        let mode = codes::mode(bits)?;
        match mode {
            codes::Mode::Pass => {
                if b2 <= at {
                    return Err(bits.error("nonadvancing CCITT pass"));
                }
                at = b2;
            }
            codes::Mode::Horizontal => {
                let middle = at + bits.run(black, width - at)?;
                let end = middle + bits.run(!black, width - middle)?;
                if end <= at {
                    return Err(bits.error("nonadvancing CCITT horizontal mode"));
                }
                changes.push(middle);
                changes.push(end);
                at = end;
            }
            codes::Mode::Vertical(delta) => {
                let end = b1
                    .checked_add_signed(isize::from(delta))
                    .filter(|&end| end >= at && end <= width)
                    .ok_or(bits.error("CCITT vertical changing element outside row"))?;
                changes.push(end);
                at = end;
                black = !black;
            }
        }
        first = false;
        if changes.len() > width + 1 {
            return Err(bits.error("too many CCITT changing elements"));
        }
    }
    Ok(())
}
