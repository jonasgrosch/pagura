//! ASCII filters (7.4.2, 7.4.3).
use crate::report::{Trailer, repair, syntax, trailing};
use crate::{Predictor, output::Output, repairs};
use pagura_core::syntax::{hex_digit, is_whitespace};
use pagura_core::{Diagnostics, Result};

/// ASCII85 digits are base-85 (7.4.3).
const BASE85: u64 = 85;
/// Digits of a full ASCII85 group.
const GROUP_DIGITS: usize = 5;
/// Bytes a full ASCII85 group encodes.
const GROUP_BYTES: usize = 4;
/// The ASCII85 digit with value 0.
const FIRST_DIGIT: u8 = b'!';
/// The ASCII85 digit with value 84, which pads a final partial group.
const LAST_DIGIT: u8 = b'u';

pub(crate) fn hex(input: &[u8], cap: usize, diagnostics: &mut Diagnostics) -> Result<Vec<u8>> {
    let mut output = Output::new(cap, Predictor::default())?;
    let mut high = None;
    let mut end = input.len();
    let mut terminated = false;
    for (offset, &byte) in input.iter().enumerate() {
        if is_whitespace(byte) {
            continue;
        }
        if byte == b'>' {
            end = offset + 1;
            terminated = true;
            break;
        }
        let digit = hex_digit(byte).ok_or_else(|| syntax("invalid ASCIIHex digit", offset))?;
        if let Some(first) = high.take() {
            output.write(&[first * 16 + digit], offset)?;
        } else {
            high = Some(digit);
        }
    }
    if !terminated {
        repair(
            diagnostics,
            repairs::TRUNCATED,
            "ASCIIHex end marker missing",
            end,
        )?;
    }
    if let Some(first) = high {
        output.write(&[first * 16], end)?;
    }
    trailing(input, end, Trailer::Whitespace, diagnostics)?;
    output.finish(end)
}

pub(crate) fn ascii85(input: &[u8], cap: usize, diagnostics: &mut Diagnostics) -> Result<Vec<u8>> {
    let mut output = Output::new(cap, Predictor::default())?;
    let mut group = 0_u64;
    let mut count = 0;
    let mut end = input.len();
    let mut terminated = false;
    for (offset, &byte) in input.iter().enumerate() {
        if is_whitespace(byte) {
            continue;
        }
        if byte == b'~' {
            // 7.4.3 ignores all PDF whitespace, including within EOD.
            let next = input
                .get(offset + 1..)
                .and_then(|tail| tail.iter().position(|&byte| !is_whitespace(byte)))
                .map(|index| offset + 1 + index);
            match next.and_then(|index| input.get(index).copied().map(|byte| (index, byte))) {
                Some((index, b'>')) => {
                    end = index + 1;
                    terminated = true;
                    break;
                }
                None => break,
                Some((index, _)) => return Err(syntax("invalid ASCII85 end marker", index)),
            }
        }
        if byte == b'z' {
            if count != 0 {
                return Err(syntax("ASCII85 z inside a group", offset));
            }
            output.write(&[0; GROUP_BYTES], offset)?;
            continue;
        }
        if !(FIRST_DIGIT..=LAST_DIGIT).contains(&byte) {
            return Err(syntax("invalid ASCII85 digit", offset));
        }
        group = group * BASE85 + u64::from(byte - FIRST_DIGIT);
        count += 1;
        if count == GROUP_DIGITS {
            write_group(&mut output, group, GROUP_BYTES, offset)?;
            group = 0;
            count = 0;
        }
    }
    if count == 1 {
        return Err(syntax("ASCII85 final group has one digit", end));
    }
    if !terminated {
        repair(
            diagnostics,
            repairs::TRUNCATED,
            "ASCII85 end marker missing",
            end,
        )?;
    }
    if count > 1 {
        for _ in count..GROUP_DIGITS {
            group = group * BASE85 + u64::from(LAST_DIGIT - FIRST_DIGIT);
        }
        write_group(&mut output, group, count - 1, end)?;
    }
    trailing(input, end, Trailer::Whitespace, diagnostics)?;
    output.finish(end)
}

fn write_group(output: &mut Output, group: u64, count: usize, offset: usize) -> Result<()> {
    let bytes = u32::try_from(group)
        .map_err(|_| syntax("ASCII85 group overflow", offset))?
        .to_be_bytes();
    output.write(
        bytes
            .get(..count)
            .ok_or_else(|| syntax("invalid ASCII85 group size", offset))?,
        offset,
    )
}
