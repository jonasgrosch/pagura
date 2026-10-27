//! ASCII and run-length filters (ISO 32000-2 7.4.2, 7.4.3, 7.4.5).
use crate::{Predictor, output::Output, repair, syntax, tags, trailing};
use pagura_core::syntax::is_whitespace;
use pagura_core::{Diagnostics, Result};

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
        let digit = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            _ => {
                return crate::damaged(
                    syntax("invalid ASCIIHex digit", offset),
                    diagnostics,
                    || output.complete(),
                );
            }
        };
        if let Some(first) = high.take() {
            output.write(&[first * 16 + digit], offset)?;
        } else {
            high = Some(digit);
        }
    }
    if !terminated {
        repair(
            diagnostics,
            tags::TRUNCATED,
            "ASCIIHex end marker missing",
            end,
        )?;
    }
    if let Some(first) = high {
        output.write(&[first * 16], end)?;
    }
    trailing(input, end, true, diagnostics)?;
    output.finish(end, diagnostics)
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
            // Clause 7.4.3 ignores all PDF whitespace, including within EOD.
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
                Some((index, _)) => {
                    return crate::damaged(
                        syntax("invalid ASCII85 end marker", index),
                        diagnostics,
                        || output.complete(),
                    );
                }
            }
        }
        if byte == b'z' {
            if count != 0 {
                return crate::damaged(
                    syntax("ASCII85 z inside a group", offset),
                    diagnostics,
                    || output.complete(),
                );
            }
            output.write(&[0; 4], offset)?;
            continue;
        }
        if !(b'!'..=b'u').contains(&byte) {
            return crate::damaged(syntax("invalid ASCII85 digit", offset), diagnostics, || {
                output.complete()
            });
        }
        group = group * 85 + u64::from(byte - b'!');
        count += 1;
        if count == 5 {
            write_group(&mut output, group, 4, offset)?;
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
            tags::TRUNCATED,
            "ASCII85 end marker missing",
            end,
        )?;
    }
    if count > 1 {
        for _ in count..5 {
            group = group * 85 + 84;
        }
        write_group(&mut output, group, count - 1, end)?;
    }
    trailing(input, end, true, diagnostics)?;
    output.finish(end, diagnostics)
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

pub(crate) fn run_length(
    input: &[u8],
    cap: usize,
    diagnostics: &mut Diagnostics,
) -> Result<Vec<u8>> {
    let mut output = Output::new(cap, Predictor::default())?;
    let mut offset = 0;
    while let Some(&tag) = input.get(offset) {
        offset += 1;
        match tag {
            128 => {
                trailing(input, offset, false, diagnostics)?;
                return output.finish(offset, diagnostics);
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
        tags::TRUNCATED,
        "RunLength input truncated",
        input.len(),
    )?;
    output.finish(input.len(), diagnostics)
}
