//! The run-length filter (7.4.5).
use crate::report::{Trailer, repair, syntax, trailing};
use crate::{Predictor, output::Output, repairs};
use pagura_core::{Diagnostics, Result};

/// The length byte that ends the data.
const EOD: u8 = 128;
/// Length bytes up to this one copy the next `length + 1` bytes.
const LAST_LITERAL: u8 = 127;
/// A length byte above `EOD` repeats the next byte `REPEAT - length` times.
const REPEAT: usize = 257;
/// The longest run either kind of packet produces.
const LONGEST: usize = 128;

pub(crate) fn decode(input: &[u8], cap: usize, diagnostics: &mut Diagnostics) -> Result<Vec<u8>> {
    let mut output = Output::new(cap, Predictor::default())?;
    let mut offset = 0;
    while let Some(&tag) = input.get(offset) {
        offset += 1;
        match tag {
            EOD => {
                trailing(input, offset, Trailer::Nothing, diagnostics)?;
                return output.finish(offset);
            }
            0..=LAST_LITERAL => {
                let count = usize::from(tag) + 1;
                let available = count.min(input.len() - offset);
                output.write(
                    input
                        .get(offset..offset + available)
                        .ok_or_else(|| syntax("invalid run length", offset))?,
                    offset,
                )?;
                offset += available;
                if available != count {
                    break;
                }
            }
            _ => {
                let Some(&byte) = input.get(offset) else {
                    break;
                };
                output.write(&[byte; LONGEST][..REPEAT - usize::from(tag)], offset)?;
                offset += 1;
            }
        }
    }
    repair(
        diagnostics,
        repairs::TRUNCATED,
        "RunLength input truncated",
        input.len(),
    )?;
    output.finish(input.len())
}
