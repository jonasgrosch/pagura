//! The run-length filter (ISO 32000-2 7.4.5).
use crate::report::{repair, syntax, trailing};
use crate::{Predictor, output::Output, repairs};
use pagura_core::{Diagnostics, Result};

pub(crate) fn decode(input: &[u8], cap: usize, diagnostics: &mut Diagnostics) -> Result<Vec<u8>> {
    let mut output = Output::new(cap, Predictor::default())?;
    let mut offset = 0;
    while let Some(&tag) = input.get(offset) {
        offset += 1;
        match tag {
            128 => {
                trailing(input, offset, false, diagnostics)?;
                return output.finish(offset);
            }
            0..=127 => {
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
                output.write(&[byte; 128][..257 - usize::from(tag)], offset)?;
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
