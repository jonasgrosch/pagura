//! Canonical Huffman decode tables (RFC 1951 3.2.2) with combined entries.
//!
//! One lookup yields what a code decodes to: a literal byte, a length or
//! distance base with its extra-bit count, the end of the block, or a link to
//! a second-level table for codes longer than the primary index. Primary
//! tables index at most [`LITLEN_BITS`] or [`DISTANCE_BITS`] bits, so building
//! one costs about one write per primary slot instead of 2^15.
//!
//! An entry packs, from the least significant bit:
//! - bits 0..=7: bits a match consumes (code plus extra bits), or the code
//!   length for literals and the end of the block;
//! - bits 8..=11: the code length, where extra bits begin; for a link, the
//!   second-level index width;
//! - bits 12..=15: [`CODE_284`], [`END_OF_BLOCK`], [`LINK`], [`EXCEPTIONAL`];
//! - bits 16..=30: the literal, base or second-level start;
//! - bit 31: [`LITERAL`].
use super::bits::{Bits, low_bits};
use super::{DIST_BASE, DIST_EXTRA, Fault, InflateResult, LENGTH_BASE, LENGTH_EXTRA};
use crate::syntax;
use pagura_core::Result;

pub(super) const LITERAL: u32 = 1 << 31;
/// Not a literal or match: the end of the block, a link, a reserved symbol
/// (with its code length) or a slot no code reaches (code length zero).
pub(super) const EXCEPTIONAL: u32 = 1 << 15;
pub(super) const LINK: u32 = 1 << 14;
pub(super) const END_OF_BLOCK: u32 = 1 << 13;
/// Length code 284, whose extra bits must not reach length 258 (RFC 1951 3.2.5).
pub(super) const CODE_284: u32 = 1 << 12;
const UNREACHED: u32 = EXCEPTIONAL;

/// Primary index widths. Second-level tables take the remaining code bits.
pub(super) const LITLEN_BITS: u32 = 11;
pub(super) const DISTANCE_BITS: u32 = 8;
const PRECODE_BITS: u32 = 7;

// The most entries a complete code can need with these primary widths:
// 2342 for 288 symbols, 402 for 32 (zlib's `enough` bound for codes of at
// most 15 bits), and the whole 7-bit table for the 19 code-length symbols.
pub(super) const LITLEN_ENTRIES: usize = 2342;
pub(super) const DISTANCE_ENTRIES: usize = 402;
pub(super) const PRECODE_ENTRIES: usize = 1 << PRECODE_BITS;

pub(super) type LitlenTable = Table<LITLEN_BITS, LITLEN_ENTRIES>;
pub(super) type DistanceTable = Table<DISTANCE_BITS, DISTANCE_ENTRIES>;
pub(super) type PrecodeTable = Table<PRECODE_BITS, PRECODE_ENTRIES>;

/// Bits a match entry consumes, code and extra bits together.
pub(super) fn total(entry: u32) -> u32 {
    entry & 0xff
}

pub(super) fn code_length(entry: u32) -> u32 {
    (entry >> 8) & 0xf
}

pub(super) fn value(entry: u32) -> u32 {
    (entry >> 16) & 0x7fff
}

/// A match entry's length or distance: its base plus the extra bits that
/// follow its code in `buffer`.
pub(super) fn base_plus_extra(entry: u32, buffer: u64) -> usize {
    // At most 28 bits are masked, a 15-bit code and 13 extra bits, and
    // the sum stays below 2^16.
    value(entry).wrapping_add(low_bits(buffer, total(entry)) >> code_length(entry)) as usize
}

/// A decode table with a primary index of at most `BITS` bits and room for
/// `N` entries, primary and second-level together.
pub(super) struct Table<const BITS: u32, const N: usize> {
    entries: [u32; N],
    /// Primary index mask; the primary table has `mask + 1` entries.
    mask: usize,
    /// Primary index width.
    bits: u32,
    /// Longest code length; zero for an empty alphabet.
    width: u32,
}

impl<const BITS: u32, const N: usize> Table<BITS, N> {
    // Masking with the largest primary index lets lookups skip bounds checks.
    const PRIMARY_LIMIT: usize = (1 << BITS) - 1;

    pub(super) fn new() -> Self {
        Self {
            entries: [UNREACHED; N],
            mask: 0,
            bits: 0,
            width: 0,
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.width == 0
    }

    /// The primary entry for the next bits; it may be a [`LINK`].
    #[inline]
    pub(super) fn primary(&self, buffer: u64) -> u32 {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the index is masked to at most BITS bits"
        )]
        let index = buffer as usize & self.mask & Self::PRIMARY_LIMIT;
        self.entries[index]
    }

    /// Follows a [`LINK`] entry to the entry for the next bits.
    #[inline]
    pub(super) fn secondary(&self, link: u32, buffer: u64) -> u32 {
        let index = value(link) as usize
            + low_bits(buffer.wrapping_shr(self.bits), code_length(link)) as usize;
        self.entries.get(index).copied().unwrap_or(UNREACHED)
    }

    /// The entry for the next bits, through a link if there is one.
    #[inline]
    pub(super) fn lookup(&self, buffer: u64) -> u32 {
        let entry = self.primary(buffer);
        if entry & LINK == 0 {
            entry
        } else {
            self.secondary(entry, buffer)
        }
    }

    /// Decodes one code and consumes it, but not its extra bits. Near the end
    /// of the input, missing bits read as zeros; a code longer than what
    /// remains is the end of the input, as is an unreached slot that more
    /// input could have avoided.
    pub(super) fn decode(&self, bits: &mut Bits<'_>) -> InflateResult<u32> {
        if bits.count < self.width {
            bits.refill();
        }
        let entry = self.lookup(bits.buffer);
        let length = code_length(entry);
        if length == 0 {
            if bits.count < self.width {
                return Err(Fault::End);
            }
            return Err(syntax("invalid Huffman code", bits.offset()).into());
        }
        if length > bits.count {
            return Err(Fault::End);
        }
        bits.consume(length);
        Ok(entry)
    }

    /// Builds the table for canonical code `lengths` (RFC 1951 3.2.2).
    /// `symbol` gives each symbol's entry without its code length; extra-bit
    /// counts sit in the low byte. Incomplete codes are rejected except a
    /// single one-bit code when `single` allows it (RFC 1951 3.2.7).
    pub(super) fn build(
        &mut self,
        lengths: &[u8],
        single: bool,
        empty: bool,
        offset: usize,
        symbol: impl Fn(u16) -> u32,
    ) -> Result<()> {
        let Some((counts, width, complete)) = code_counts(lengths, single, offset)? else {
            if empty {
                self.entries[0] = UNREACHED;
                self.mask = 0;
                self.bits = 0;
                self.width = 0;
                return Ok(());
            }
            return Err(syntax("empty Huffman alphabet", offset));
        };
        let bits = width.min(BITS);
        let primary = 1_usize << bits;
        if !complete {
            self.entries[..primary].fill(UNREACHED);
        }
        // Symbols in canonical order: by code length, then symbol value.
        let mut start = [0_usize; 16];
        for length in 1..15 {
            start[length + 1] = start[length] + counts[length] as usize;
        }
        let mut sorted = [0_u16; 288];
        for (index, &length) in lengths.iter().enumerate() {
            if length != 0 {
                let slot = &mut start[usize::from(length)];
                if let (Some(target), Ok(index)) = (sorted.get_mut(*slot), u16::try_from(index)) {
                    *target = index;
                }
                *slot += 1;
            }
        }
        let mut left = counts;
        let mut next_link = primary;
        let (mut prefix, mut link_start, mut link_bits) = (usize::MAX, 0_usize, 0);
        let mut code = 0_u32;
        let mut position = 0;
        for length in 1..=width {
            if length <= bits {
                // Entries for shorter codes repeat every 2^(length - 1)
                // slots; this length's codes then take their own slots.
                let size = 1 << (length - 1);
                self.entries.copy_within(..size, size);
            }
            for _ in 0..counts[length as usize] {
                let entry =
                    symbol(sorted.get(position).copied().unwrap_or(0)) + (length << 8) + length;
                position += 1;
                let reversed = (code.reverse_bits() >> (32 - length)) as usize;
                if length <= bits {
                    if let Some(slot) = self.entries.get_mut(reversed) {
                        *slot = entry;
                    }
                } else {
                    if reversed & (primary - 1) != prefix {
                        // A new second-level table: as wide as needed for
                        // the remaining codes under this prefix to fill it.
                        prefix = reversed & (primary - 1);
                        link_start = next_link;
                        link_bits = link_width(&left, length, bits, width);
                        next_link += 1 << link_bits;
                        if next_link > N {
                            return Err(syntax("Huffman alphabet too large", offset));
                        }
                        let start = u32::try_from(link_start)
                            .map_err(|_| syntax("Huffman alphabet too large", offset))?;
                        self.entries[prefix] =
                            EXCEPTIONAL | LINK | (start << 16) | (link_bits << 8);
                    }
                    let table = &mut self.entries[link_start..link_start + (1 << link_bits)];
                    for slot in table
                        .iter_mut()
                        .skip(reversed >> bits)
                        .step_by(1 << (length - bits))
                    {
                        *slot = entry;
                    }
                }
                left[length as usize] -= 1;
                code += 1;
            }
            code <<= 1;
        }
        self.mask = primary - 1;
        self.bits = bits;
        self.width = width;
        Ok(())
    }
}

/// Counts codes of each length and checks that they form a prefix code
/// (RFC 1951 3.2.2). Returns the counts, the longest length and whether the
/// code is complete, or `None` for an alphabet without codes.
fn code_counts(
    lengths: &[u8],
    single: bool,
    offset: usize,
) -> Result<Option<([u32; 16], u32, bool)>> {
    let width = lengths.iter().copied().max().unwrap_or(0);
    if width > 15 {
        return Err(syntax("invalid Huffman code length", offset));
    }
    if width == 0 {
        return Ok(None);
    }
    // Four histograms, so consecutive equal lengths do not wait on each
    // other's increments.
    let mut partial = [[0_u32; 16]; 4];
    let (quads, rest) = lengths.as_chunks::<4>();
    for quad in quads {
        for (histogram, &length) in partial.iter_mut().zip(quad) {
            histogram[usize::from(length & 15)] += 1;
        }
    }
    for &length in rest {
        partial[0][usize::from(length & 15)] += 1;
    }
    let counts: [u32; 16] =
        std::array::from_fn(|length| partial.iter().map(|histogram| histogram[length]).sum());
    let width = u32::from(width);
    let mut remaining = 1_i64;
    for &count in counts.iter().skip(1) {
        remaining = remaining * 2 - i64::from(count);
        if remaining < 0 {
            return Err(syntax("oversubscribed Huffman alphabet", offset));
        }
    }
    // RFC 1951 3.2.7 explicitly allows a one-bit single distance code.
    // General incomplete-tree rejection is decoder policy: 3.2.1/3.2.2
    // require prefix/canonical codes but do not explicitly require a
    // complete tree. The same one-symbol exception permits EOB-only blocks.
    if remaining != 0 && !(single && width == 1 && counts[1] == 1) {
        return Err(syntax("incomplete Huffman alphabet", offset));
    }
    Ok(Some((counts, width, remaining == 0)))
}

/// The index width of a second-level table whose first code has `length`
/// bits: the least that the codes still `left` under its prefix fill.
fn link_width(left: &[u32; 16], length: u32, bits: u32, width: u32) -> u32 {
    let mut extra = length - bits;
    let mut room = 1_i64 << extra;
    while bits + extra < width {
        room -= i64::from(left[(bits + extra) as usize]);
        if room <= 0 {
            break;
        }
        extra += 1;
        room <<= 1;
    }
    extra
}

/// Literal/length symbols (RFC 1951 3.2.5); 286 and 287 are reserved.
pub(super) fn litlen_symbol(symbol: u16) -> u32 {
    let index = usize::from(symbol.wrapping_sub(257));
    match (symbol, LENGTH_BASE.get(index), LENGTH_EXTRA.get(index)) {
        (0..=255, ..) => LITERAL | (u32::from(symbol) << 16),
        (256, ..) => EXCEPTIONAL | END_OF_BLOCK,
        (_, Some(&base), Some(&extra)) => {
            let flag = if symbol == 284 { CODE_284 } else { 0 };
            u32::try_from(base).map_or(EXCEPTIONAL, |base| (base << 16) | flag | u32::from(extra))
        }
        _ => EXCEPTIONAL,
    }
}

/// Distance symbols (RFC 1951 3.2.5); 30 and 31 are reserved.
pub(super) fn distance_symbol(symbol: u16) -> u32 {
    let index = usize::from(symbol);
    match (DIST_BASE.get(index), DIST_EXTRA.get(index)) {
        (Some(&base), Some(&extra)) => {
            u32::try_from(base).map_or(EXCEPTIONAL, |base| (base << 16) | u32::from(extra))
        }
        _ => EXCEPTIONAL,
    }
}

/// Code-length symbols 0..=18 (RFC 1951 3.2.7).
pub(super) fn precode_symbol(symbol: u16) -> u32 {
    u32::from(symbol) << 16
}

pub(super) fn litlen(
    table: &mut LitlenTable,
    lengths: &[u8],
    single: bool,
    offset: usize,
) -> Result<()> {
    table.build(lengths, single, false, offset, litlen_symbol)
}

pub(super) fn distance(
    table: &mut DistanceTable,
    lengths: &[u8],
    dynamic: bool,
    offset: usize,
) -> Result<()> {
    table.build(lengths, dynamic, dynamic, offset, distance_symbol)
}

pub(super) fn precode(table: &mut PrecodeTable, lengths: &[u8], offset: usize) -> Result<()> {
    table.build(lengths, false, false, offset, precode_symbol)
}
