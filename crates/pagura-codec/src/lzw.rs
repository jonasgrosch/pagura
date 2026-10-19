//! PDF LZW: MSB-first codes of 9 to 12 bits with `/EarlyChange` (7.4.4.2).
use crate::report::{Trailer, repair, syntax, trailing};
use crate::{EarlyChange, Predictor, output::Output, repairs};
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
    let mut reader = Codes {
        input,
        offset: 0,
        buffer: 0,
        bits: 0,
    };
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
    while let Some(code) = reader.read(width) {
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
            EOD => return end(&reader, input, output, diagnostics),
            _ => {
                // The encoder inserts one entry ahead while more input remains.
                // At decoder next=4095, another data code would mean the encoder
                // continued after filling its table instead of issuing clear
                // (7.4.4.2). EOD can still finish without that insertion.
                if table.next >= LAST && !full_table_reported {
                    repair(
                        diagnostics,
                        repairs::LZW_FULL_TABLE,
                        "LZW full table not cleared; dictionary frozen",
                        reader.position(),
                    )?;
                    full_table_reported = true;
                }
                let special = code == table.next && previous.is_some();
                let source = if special { previous.unwrap_or(0) } else { code };
                if source >= table.next || source == CLEAR || source == EOD {
                    return Err(syntax("invalid LZW dictionary code", reader.position()));
                }
                let index = usize::from(source);
                let length = usize::from(table.lengths.get(index).copied().unwrap_or(0));
                output.check(length + usize::from(special), reader.position())?;
                let first = table.first.get(index).copied().unwrap_or(0);
                let bytes = table.expand(source, &mut stack, reader.position())?;
                output.write(bytes, reader.position())?;
                if special {
                    output.write(&[first], reader.position())?;
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
    reader: &Codes<'_>,
    input: &[u8],
    output: Output,
    diagnostics: &mut Diagnostics,
) -> Result<Vec<u8>> {
    if reader.buffer & ((1 << reader.bits) - 1) != 0 {
        repair(
            diagnostics,
            repairs::LZW_PADDING,
            "nonzero LZW end padding ignored",
            reader.position(),
        )?;
    }
    trailing(input, reader.offset, Trailer::Nothing, diagnostics)?;
    output.finish(reader.offset)
}

/// MSB-first codes of at most 12 bits from a refill of one byte at a time.
///
/// This stays crate-local rather than using `pagura_core::bytes::BitCursor`
/// (an exception to the binary-input convention, like DEFLATE's LSB-first
/// reader): a measured comparison found decoding through `BitCursor` 8–20%
/// slower on every LZW workload, with identical output.
struct Codes<'a> {
    input: &'a [u8],
    offset: usize,
    buffer: u32,
    bits: u8,
}
impl Codes<'_> {
    fn read(&mut self, width: u8) -> Option<u16> {
        while self.bits < width {
            self.buffer = (self.buffer << 8) | u32::from(*self.input.get(self.offset)?);
            self.offset += 1;
            self.bits += 8;
        }
        self.bits -= width;
        u16::try_from((self.buffer >> self.bits) & ((1 << width) - 1)).ok()
    }
    fn position(&self) -> usize {
        self.offset.saturating_sub(1)
    }
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
