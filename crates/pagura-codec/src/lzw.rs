//! PDF LZW, MSB-first codes and EarlyChange (ISO 32000-2 7.4.4.2).
use crate::report::{repair, syntax, trailing};
use crate::{Predictor, output::Output, repairs};
use pagura_core::{Diagnostics, Result};

pub(crate) fn decode(
    input: &[u8],
    cap: usize,
    predictor: Predictor,
    early: bool,
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
    let mut width = 9;
    let mut previous = None;
    let mut first_code = true;
    let mut full_table_reported = false;
    let mut stack = [0; 4096];
    while let Some(code) = reader.read(width) {
        if first_code && code != 256 {
            if code > 257 {
                return Err(syntax("LZW must start with clear code", 0));
            }
            repair(
                diagnostics,
                repairs::LZW_INITIAL_CLEAR,
                "LZW initial clear code missing; initial dictionary retained (7.4.4.2)",
                0,
            )?;
        }
        first_code = false;
        match code {
            256 => {
                table.next = 258;
                width = 9;
                previous = None;
            }
            257 => {
                // ISO 32000-2 7.4.4.2: unused final-byte bits are zero.
                if reader.buffer & ((1 << reader.bits) - 1) != 0 {
                    repair(
                        diagnostics,
                        repairs::LZW_PADDING,
                        "nonzero LZW end padding ignored",
                        reader.position(),
                    )?;
                }
                trailing(input, reader.offset, false, diagnostics)?;
                return output.finish(reader.offset);
            }
            _ => {
                // The encoder inserts one entry ahead while more input remains.
                // At decoder next=4095, another data code would mean the encoder
                // continued after filling its table instead of issuing clear
                // (ISO 32000-2 7.4.4.2). EOD can still finish without that insertion.
                if table.next >= 4095 && !full_table_reported {
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
                if source >= table.next || source == 256 || source == 257 {
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
                    if width < 12 && table.next == (1 << width) - u16::from(early) {
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
    prefix: [u16; 4096],
    suffix: [u8; 4096],
    first: [u8; 4096],
    lengths: [u16; 4096],
    next: u16,
}
impl Table {
    fn new() -> Self {
        let mut table = Self {
            prefix: [0; 4096],
            suffix: [0; 4096],
            first: [0; 4096],
            lengths: [0; 4096],
            next: 258,
        };
        for byte in 0..=255_u8 {
            let index = usize::from(byte);
            table.suffix[index] = byte;
            table.first[index] = byte;
            table.lengths[index] = 1;
        }
        table
    }
    fn add(&mut self, previous: u16, first: u8) {
        if self.next >= 4096 {
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
        stack: &'a mut [u8; 4096],
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
            if code < 256 {
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
