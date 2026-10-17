//! RFC 4013 over RFC 3454, Unicode 3.2, for ISO 32000-2 7.6.4.3.3(a).
//!
//! Mapping precedes NFKC; output checks precede bidi checks (RFC 3454 section 2).
//! Until normalization data is available, only mapped ASCII is known to be
//! normalized. Other input returns the existing explicit unsupported feature.
use pagura_core::{Error, Position, Result};
use std::borrow::Cow;

mod normalization;
mod tables;

const PROHIBITED: u16 = tables::C_1_2
    | tables::C_2_1
    | tables::C_2_2
    | tables::C_3
    | tables::C_4
    | tables::C_5
    | tables::C_6
    | tables::C_7
    | tables::C_8
    | tables::C_9;

fn membership(code: u32) -> u16 {
    let index = tables::RANGES.partition_point(|&(_, end, _)| end < code);
    tables::RANGES
        .get(index)
        .filter(|&&(start, _, _)| start <= code)
        .map_or(0, |&(_, _, flags)| flags)
}

/// RFC 4013 section 2.1; B.1 removal takes precedence over C.1.2 spacing
/// when a code point belongs to both tables (U+200B).
fn map(input: &str) -> Result<Cow<'_, str>> {
    let Some((first, _)) = input
        .char_indices()
        .find(|&(_, ch)| membership(u32::from(ch)) & (tables::B_1 | tables::C_1_2) != 0)
    else {
        return Ok(Cow::Borrowed(input));
    };
    // Both mappings shrink UTF-8, so this is an upper bound on output capacity.
    let mut mapped = String::new();
    mapped
        .try_reserve_exact(input.len())
        .map_err(|_| Error::limit("cannot allocate SASLprep mapping buffer"))?;
    mapped.push_str(&input[..first]);
    for ch in input[first..].chars() {
        let flags = membership(u32::from(ch));
        if flags & tables::B_1 == 0 {
            mapped.push(if flags & tables::C_1_2 != 0 { ' ' } else { ch });
        }
    }
    Ok(Cow::Owned(mapped))
}

/// Checks already normalized output (RFC 4013 sections 2.3-2.5).
///
/// A.1 is rejected using stored-string rules; applying the same restriction to
/// authentication queries is permitted by RFC 3454 section 7. Positions refer
/// to bytes in the normalized string, not the PDF or the original password.
fn validate_output(output: &str) -> Result<()> {
    let mut randal = None;
    let mut has_lcat = false;
    let mut first_flags = None;
    let mut last_flags = 0;
    for (offset, ch) in output.char_indices() {
        let flags = membership(u32::from(ch));
        first_flags.get_or_insert(flags);
        last_flags = flags;
        if flags & PROHIBITED != 0 {
            return Err(
                Error::password("password contains a SASLprep-prohibited code point")
                    .at(Position::Buffer(offset)),
            );
        }
        if flags & tables::A_1 != 0 {
            return Err(
                Error::password("password contains an unassigned Unicode 3.2 code point")
                    .at(Position::Buffer(offset)),
            );
        }
        if flags & tables::D_1 != 0 {
            randal.get_or_insert(offset);
        }
        has_lcat |= flags & tables::D_2 != 0;
    }
    // RFC 3454 section 6: RandAL excludes LCat, and must occupy both ends.
    if let Some(offset) = randal
        && (has_lcat
            || first_flags.is_none_or(|flags| flags & tables::D_1 == 0)
            || last_flags & tables::D_1 == 0)
    {
        return Err(
            Error::password("password violates the SASLprep bidirectional rule")
                .at(Position::Buffer(offset)),
        );
    }
    Ok(())
}

pub(super) fn prepare(input: &str) -> Result<Cow<'_, str>> {
    let normalized = normalization::nfkc32(map(input)?)?;
    validate_output(&normalized)?;
    Ok(normalized)
}

#[cfg(test)]
mod tests;
