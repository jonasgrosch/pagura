//! PDF LZW: MSB-first codes of 9 to 12 bits with `/EarlyChange` (7.4.4.2).
use crate::report::{Trailer, repair, syntax, trailing};
use crate::{EarlyChange, Predictor, output::Output, repairs};
use pagura_core::bytes::{BitCursor, Cursor};
use pagura_core::{Diagnostics, Result};

/// Resets the table and the code width.
const CLEAR: u16 = 256;
/// Ends the data.
const EOD: u16 = 257;
/// The first code a cleared table assigns.
const FIRST_FREE: u16 = 258;
/// The last code; once it is assigned the table is full.
const LAST: u16 = 4095;
/// Table entries, one per 12-bit code.
const TABLE: usize = 4096;
/// The code width after a clear.
const MIN_WIDTH: u8 = 9;
/// The widest code.
const MAX_WIDTH: u8 = 12;

pub(crate) fn decode(
    input: &[u8],
    cap: usize,
    predictor: Predictor,
    early_change: EarlyChange,
    diagnostics: &mut Diagnostics,
) -> Result<Vec<u8>> {
    let mut output = Output::new(cap, predictor)?;
    let mut reader = Codes::new(input);
    let mut table = Table::new();
    let mut width = MIN_WIDTH;
    let mut previous = None;
    let mut first_code = true;
    let mut full_table_reported = false;
    let mut stack = [0; TABLE];
    let lead = match early_change {
        EarlyChange::Early => 1,
        EarlyChange::Postponed => 0,
    };
    while let Some(code) = reader.read(width)? {
        if first_code && code != CLEAR {
            // No table entry exists yet for a code above EOD.
            if code > EOD {
                return Err(syntax("LZW must start with clear code", 0));
            }
            // A literal or EOD reads the same from the initial table.
            repair(
                diagnostics,
                repairs::LZW_INITIAL_CLEAR,
                "LZW initial clear code missing; initial dictionary retained (7.4.4.2)",
                0,
            )?;
        }
        first_code = false;
        match code {
            CLEAR => {
                table.next = FIRST_FREE;
                width = MIN_WIDTH;
                previous = None;
            }
            EOD => return end(reader, input, output, diagnostics),
            _ => {
                let at = reader.position();
                // The encoder inserts one entry ahead while more input remains.
                // At decoder next=4095, another data code would mean the encoder
                // continued after filling its table instead of issuing clear
                // (7.4.4.2). EOD can still finish without that insertion.
                if table.next >= LAST && !full_table_reported {
                    repair(
                        diagnostics,
                        repairs::LZW_FULL_TABLE,
                        "LZW full table not cleared; dictionary frozen",
                        at,
                    )?;
                    full_table_reported = true;
                }
                let special = code == table.next && previous.is_some();
                let source = if special { previous.unwrap_or(0) } else { code };
                if source >= table.next || source == CLEAR || source == EOD {
                    return Err(syntax("invalid LZW dictionary code", at));
                }
                let index = usize::from(source);
                let length = usize::from(table.lengths.get(index).copied().unwrap_or(0));
                output.check(length + usize::from(special), at)?;
                let first = table.first.get(index).copied().unwrap_or(0);
                let bytes = table.expand(source, &mut stack, at)?;
                output.write(bytes, at)?;
                if special {
                    output.write(&[first], at)?;
                }
                if let Some(prev) = previous {
                    // Complete the one pending decoder entry (4095), then
                    // Table::add freezes the full dictionary until clear.
                    table.add(prev, first);
                    if width < MAX_WIDTH && table.next == (1 << width) - lead {
                        width += 1;
                    }
                }
                previous = Some(code);
            }
        }
    }
    repair(
        diagnostics,
        repairs::TRUNCATED,
        "LZW end code missing",
        input.len(),
    )?;
    output.finish(input.len())
}

/// Finishes at EOD: the rest of its byte must be zero (7.4.4.2) and nothing
/// may follow.
fn end(
    mut reader: Codes<'_>,
    input: &[u8],
    output: Output,
    diagnostics: &mut Diagnostics,
) -> Result<Vec<u8>> {
    let padding = reader.position();
    if reader.bits.align()?.value() != 0 {
        repair(
            diagnostics,
            repairs::LZW_PADDING,
            "nonzero LZW end padding ignored",
            padding,
        )?;
    }
    let end = reader.position();
    trailing(input, end, Trailer::Nothing, diagnostics)?;
    output.finish(end)
}

/// Codes read most significant bit first through a buffered bit cursor.
struct Codes<'a> {
    bits: BitCursor<'a>,
    len: usize,
}

impl<'a> Codes<'a> {
    const fn new(input: &'a [u8]) -> Self {
        Self {
            bits: BitCursor::new(Cursor::new(input)),
            len: input.len(),
        }
    }

    /// The next code, or `None` when fewer than `width` bits remain.
    fn read(&mut self, width: u8) -> Result<Option<u16>> {
        if self.bits.remaining_bits() < usize::from(width) {
            return Ok(None);
        }
        Ok(Some(code(self.bits.bits(u32::from(width))?)))
    }

    /// The input offset of the byte holding the next unread bit, from the
    /// cursor's remaining bits, which a `Plain` cursor counts exactly.
    fn position(&self) -> usize {
        self.len
            .saturating_sub(self.bits.remaining_bits().div_ceil(8))
    }
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "LZW codes have at most 12 bits"
)]
const fn code(value: u32) -> u16 {
    value as u16
}

struct Table {
    prefix: [u16; TABLE],
    suffix: [u8; TABLE],
    first: [u8; TABLE],
    lengths: [u16; TABLE],
    next: u16,
}

impl Table {
    fn new() -> Self {
        let mut table = Self {
            prefix: [0; TABLE],
            suffix: [0; TABLE],
            first: [0; TABLE],
            lengths: [0; TABLE],
            next: FIRST_FREE,
        };
        for byte in 0..=u8::MAX {
            let index = usize::from(byte);
            table.suffix[index] = byte;
            table.first[index] = byte;
            table.lengths[index] = 1;
        }
        table
    }

    fn add(&mut self, previous: u16, first: u8) {
        if self.next > LAST {
            return;
        }
        let index = usize::from(self.next);
        self.prefix[index] = previous;
        self.suffix[index] = first;
        self.first[index] = self.first.get(usize::from(previous)).copied().unwrap_or(0);
        self.lengths[index] = self
            .lengths
            .get(usize::from(previous))
            .copied()
            .unwrap_or(0)
            + 1;
        self.next += 1;
    }

    fn expand<'a>(
        &self,
        mut code: u16,
        stack: &'a mut [u8; TABLE],
        offset: usize,
    ) -> Result<&'a [u8]> {
        let mut cursor = stack.len();
        loop {
            cursor = cursor
                .checked_sub(1)
                .ok_or_else(|| syntax("cyclic LZW dictionary", offset))?;
            let byte = self
                .suffix
                .get(usize::from(code))
                .copied()
                .ok_or_else(|| syntax("invalid LZW code", offset))?;
            if let Some(slot) = stack.get_mut(cursor) {
                *slot = byte;
            }
            if code < CLEAR {
                break;
            }
            code = self
                .prefix
                .get(usize::from(code))
                .copied()
                .ok_or_else(|| syntax("invalid LZW prefix", offset))?;
        }
        stack
            .get(cursor..)
            .ok_or_else(|| syntax("invalid LZW expansion", offset))
    }
}
