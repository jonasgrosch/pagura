//! Unicode 3.2 NFKC (RFC 4013 section 2.2; RFC 3454 sections 4 and 8.1).
//!
//! RFC 3454 cites UAX #15 revision 22, version 3.2.0. The bundled data already
//! includes the F951 correction introduced in that version. Corrections from
//! later Unicode versions are deliberately absent, matching `ucd_3_2_0`.
use pagura_core::{Error, Result};
use std::borrow::Cow;

use super::discard;
use crate::secret::wipe;

mod tables;

const S_BASE: u32 = 0xac00;
const L_BASE: u32 = 0x1100;
const V_BASE: u32 = 0x1161;
const T_BASE: u32 = 0x11a7;
const L_COUNT: u32 = 19;
const V_COUNT: u32 = 21;
const T_COUNT: u32 = 28;
const N_COUNT: u32 = V_COUNT * T_COUNT;
const S_COUNT: u32 = L_COUNT * N_COUNT;

fn combining(ch: char) -> u8 {
    let code = u32::from(ch);
    let index = tables::COMBINING.partition_point(|&(_, end, _)| end < code);
    tables::COMBINING
        .get(index)
        .filter(|&&(start, _, _)| start <= code)
        .map_or(0, |&(_, _, class)| class)
}

fn quick_check(input: &str) -> bool {
    let mut previous = 0;
    for ch in input.chars() {
        let code = u32::from(ch);
        let index = tables::QUICK_CHECK.partition_point(|&(_, end)| end < code);
        if tables::QUICK_CHECK
            .get(index)
            .is_some_and(|&(start, _)| start <= code)
        {
            return false;
        }
        let class = combining(ch);
        if class != 0 && previous > class {
            return false;
        }
        previous = class;
    }
    true
}

fn allocation_error() -> Error {
    Error::limit("cannot allocate Unicode 3.2 normalization buffer")
}

fn append(output: &mut Vec<char>, ch: char) -> Result<()> {
    output.try_reserve(1).map_err(|_| allocation_error())?;
    output.push(ch);
    Ok(())
}

fn decompose(input: &str) -> Result<Vec<char>> {
    let mut output = Vec::new();
    for ch in input.chars() {
        let code = u32::from(ch);
        if (S_BASE..S_BASE + S_COUNT).contains(&code) {
            let index = code - S_BASE;
            // Every value lies in the Hangul Jamo block, so none is filtered.
            for jamo in [L_BASE + index / N_COUNT, V_BASE + index % N_COUNT / T_COUNT]
                .into_iter()
                .chain((!index.is_multiple_of(T_COUNT)).then_some(T_BASE + index % T_COUNT))
                .filter_map(char::from_u32)
            {
                append(&mut output, jamo)?;
            }
        } else if let Ok(index) = tables::DECOMPOSITIONS.binary_search_by_key(&ch, |&(c, _, _)| c) {
            let (_, offset, length) = tables::DECOMPOSITIONS[index];
            let start = usize::from(offset);
            // The generator checks that every entry lies on character boundaries.
            let sequence = tables::TEXT.get(start..start + usize::from(length));
            debug_assert!(sequence.is_some(), "decomposition {ch:?} outside the pool");
            let sequence = sequence.unwrap_or_default();
            // The generator recursively expands compatibility decompositions.
            for item in sequence.chars() {
                append(&mut output, item)?;
            }
        } else {
            append(&mut output, ch)?;
        }
    }
    Ok(output)
}

/// Stable counting sort of each nonstarter run: linear even for hostile runs
/// with arbitrarily many descending combining classes. Scratch space is reused.
fn reorder(output: &mut [char]) -> Result<()> {
    let mut scratch = Vec::new();
    let mut start = 0;
    while start < output.len() {
        if combining(output[start]) == 0 {
            start += 1;
            continue;
        }
        let mut end = start;
        let mut previous = 0;
        let mut ordered = true;
        let mut counts = [0_usize; 256];
        while end < output.len() {
            let class = combining(output[end]);
            if class == 0 {
                break;
            }
            ordered &= previous <= class;
            previous = class;
            counts[usize::from(class)] += 1;
            end += 1;
        }
        if !ordered {
            scratch.clear();
            scratch
                .try_reserve(end - start)
                .map_err(|_| allocation_error())?;
            scratch.resize(end - start, '\0');
            let mut offset = 0;
            for count in &mut counts {
                let length = *count;
                *count = offset;
                offset += length;
            }
            for &ch in &output[start..end] {
                let offset = &mut counts[usize::from(combining(ch))];
                scratch[*offset] = ch;
                *offset += 1;
            }
            output[start..end].copy_from_slice(&scratch);
        }
        start = end;
    }
    wipe(&mut scratch);
    Ok(())
}

fn composite(first: char, second: char) -> Option<char> {
    let a = u32::from(first);
    let b = u32::from(second);
    if (L_BASE..L_BASE + L_COUNT).contains(&a) && (V_BASE..V_BASE + V_COUNT).contains(&b) {
        return char::from_u32(S_BASE + (a - L_BASE) * N_COUNT + (b - V_BASE) * T_COUNT);
    }
    if (S_BASE..S_BASE + S_COUNT).contains(&a)
        && (a - S_BASE).is_multiple_of(T_COUNT)
        && (T_BASE + 1..T_BASE + T_COUNT).contains(&b)
    {
        return char::from_u32(a + b - T_BASE);
    }
    tables::COMPOSITIONS
        .binary_search_by_key(&(first, second), |&(a, b, _)| (a, b))
        .ok()
        .map(|index| tables::COMPOSITIONS[index].2)
}

fn compose(output: &mut Vec<char>) {
    let mut starter = None;
    let mut previous = 0;
    let mut written = 0;
    for read in 0..output.len() {
        let ch = output[read];
        let class = combining(ch);
        if let Some(index) = starter
            && (previous == 0 || previous < class)
            && let Some(composed) = composite(output[index], ch)
        {
            output[index] = composed;
            continue;
        }
        output[written] = ch;
        if class == 0 {
            starter = Some(written);
        }
        written += 1;
        previous = class;
    }
    output.truncate(written);
}

/// Normalizes the complete mapped string before SASLprep output validation and
/// PDF's 127-byte truncation. No runtime files or newer Unicode tables are used.
/// Unchanged text retains its original ownership; quick-check Yes allocates none.
pub(super) fn nfkc32(input: Cow<'_, str>) -> Result<Cow<'_, str>> {
    if input.is_ascii() || quick_check(&input) {
        return Ok(input);
    }
    let mut output = decompose(&input)?;
    let normalized = normalize(&input, &mut output);
    wipe(&mut output);
    Ok(match normalized? {
        Some(normalized) => {
            discard(input);
            Cow::Owned(normalized)
        }
        None => input,
    })
}

/// Reorders and composes the decomposed `output`; `None` if that reproduces
/// `input`.
fn normalize(input: &str, output: &mut Vec<char>) -> Result<Option<String>> {
    reorder(output)?;
    compose(output);
    if input.chars().eq(output.iter().copied()) {
        return Ok(None);
    }
    let length = output.iter().try_fold(0_usize, |length, ch| {
        length
            .checked_add(ch.len_utf8())
            .ok_or_else(allocation_error)
    })?;
    let mut normalized = String::new();
    normalized
        .try_reserve_exact(length)
        .map_err(|_| allocation_error())?;
    normalized.extend(output.iter());
    Ok(Some(normalized))
}

#[cfg(test)]
mod tests;
