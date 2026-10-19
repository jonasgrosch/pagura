//! Canonical Huffman codes (RFC 1951 3.2.2): the code assignment both
//! directions share, the decoder's lookup table, and the encoder's
//! length-limited construction (RFC 1951 3.2.7).
//!
//! Binary package-merge selects the cheapest 2*n-2 items at the last level.
//! Each selected occurrence of a leaf contributes one bit to its code length.
//! Stable weight/creation-order ties make the construction deterministic.
use super::bits::{Fault, Read, Reader};
use super::tables::FIXED_LITERAL_SYMBOLS;
use crate::bounded::{filled, reserve, with_capacity};
use crate::report::syntax;
use pagura_core::{Meter, Position, Result, ResultExt};

/// The first code of each length (RFC 1951 3.2.2, step 2), given how many
/// codes have each length. Length 0 means "no code" and takes none. The
/// lengths must not oversubscribe the code space.
fn first_codes(counts: &[u16; 16]) -> [u16; 16] {
    let mut next = [0; 16];
    for bits in 2..16 {
        next[bits] = (next[bits - 1] + counts[bits - 1]) << 1;
    }
    next
}

/// Codes of at most this many bits are decoded with one lookup in a table
/// of at most 2^10 entries; longer ones with a canonical search over the
/// remaining lengths. A direct table for 15-bit codes has 32,768 entries,
/// and a dynamic block about 22 bytes long can ask for two of them.
const PRIMARY_BITS: u8 = 10;
const PRIMARY: usize = 1 << PRIMARY_BITS;

#[derive(Debug, Clone, Copy, Default)]
struct Entry {
    symbol: u16,
    /// The code's length; zero where no code of at most the table's width
    /// starts with these bits.
    bits: u8,
}

/// The alphabet a code describes, which decides the incomplete codes it may
/// have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Alphabet {
    LiteralLength,
    Distance,
    CodeLength,
}

impl Alphabet {
    /// RFC 1951 3.2.7 explicitly allows a one-bit single distance code.
    /// General incomplete-tree rejection is decoder policy: 3.2.1/3.2.2
    /// require prefix/canonical codes but do not explicitly require a
    /// complete tree. The same one-symbol exception permits EOB-only blocks.
    const fn allows_single_code(self) -> bool {
        !matches!(self, Self::CodeLength)
    }

    /// A block that copies nothing needs no distance code at all.
    const fn allows_no_codes(self) -> bool {
        matches!(self, Self::Distance)
    }
}

/// A decoding table for one canonical code.
pub(super) struct Decoder {
    /// Indexed by the next `lookup` input bits.
    entries: Vec<Entry>,
    /// The table's width: the longest code, at most `PRIMARY_BITS`.
    lookup: u8,
    /// The longest code; zero for an alphabet without codes.
    width: u8,
    /// How many codes have each length.
    counts: [u16; 16],
    /// The first code of each length.
    first_code: [u16; 16],
    /// Where each length's symbols start in `symbols`.
    first_symbol: [u16; 16],
    /// Symbols in canonical order: by code length, then by value.
    symbols: [u16; FIXED_LITERAL_SYMBOLS],
}

impl Decoder {
    /// The decoder for code `lengths` (at most 288 symbols, lengths at most
    /// 15), or a syntax error at `offset` if they do not form a usable code.
    /// Its table's entries are charged to `work` before they are built.
    pub(super) fn new(
        lengths: &[u8],
        alphabet: Alphabet,
        work: &mut Meter,
        offset: usize,
    ) -> Result<Self> {
        debug_assert!(lengths.len() <= FIXED_LITERAL_SYMBOLS);
        let mut counts = [0_u16; 16];
        for &length in lengths {
            let slot = counts
                .get_mut(usize::from(length))
                .ok_or_else(|| syntax("invalid Huffman code length", offset))?;
            *slot += 1;
        }
        counts[0] = 0;
        let width = lengths.iter().copied().max().unwrap_or(0);
        let mut decoder = Self {
            entries: Vec::new(),
            lookup: width.min(PRIMARY_BITS),
            width,
            counts,
            first_code: [0; 16],
            first_symbol: [0; 16],
            symbols: [0; FIXED_LITERAL_SYMBOLS],
        };
        if width == 0 {
            return if alphabet.allows_no_codes() {
                Ok(decoder)
            } else {
                Err(syntax("empty Huffman alphabet", offset))
            };
        }
        let mut remaining = 1_i32;
        for &count in &counts[1..] {
            remaining = remaining * 2 - i32::from(count);
            if remaining < 0 {
                return Err(syntax("oversubscribed Huffman alphabet", offset));
            }
        }
        if remaining != 0 && !(alphabet.allows_single_code() && width == 1 && counts[1] == 1) {
            return Err(syntax("incomplete Huffman alphabet", offset));
        }
        decoder.first_code = first_codes(&counts);
        let mut start = 0;
        for (first, &count) in decoder.first_symbol.iter_mut().zip(&counts).skip(1) {
            *first = start;
            start += count;
        }
        let size = 1_usize << decoder.lookup;
        work.charge(size).at(Position::Buffer(offset))?;
        decoder.entries = filled(size, Entry::default())?;
        let mut next_symbol = decoder.first_symbol;
        let mut next_code = decoder.first_code;
        for (symbol, &length) in (0_u16..).zip(lengths) {
            if length == 0 {
                continue;
            }
            let index = usize::from(length);
            decoder.symbols[usize::from(next_symbol[index])] = symbol;
            next_symbol[index] += 1;
            let code = next_code[index];
            next_code[index] += 1;
            if length <= decoder.lookup {
                // Table indices are the code's bits in reading order.
                let reversed = usize::from(code.reverse_bits() >> (16 - length));
                for entry in decoder
                    .entries
                    .iter_mut()
                    .skip(reversed)
                    .step_by(1 << length)
                {
                    *entry = Entry {
                        symbol,
                        bits: length,
                    };
                }
            }
        }
        Ok(decoder)
    }

    pub(super) fn read(&self, bits: &mut Reader<'_>) -> Read<u16> {
        if self.width == 0 {
            return Err(syntax("distance code in empty alphabet", bits.offset).into());
        }
        let (index, available) = bits.peek(self.lookup);
        let entry = self
            .entries
            .get(index)
            .copied()
            .ok_or_else(|| syntax("invalid Huffman code", bits.offset))?;
        if entry.bits == 0 {
            if self.width > self.lookup {
                return self.read_long(bits);
            }
            if available < self.lookup {
                return Err(Fault::End);
            }
            return Err(syntax("invalid Huffman code", bits.offset).into());
        }
        if entry.bits > available {
            return Err(Fault::End);
        }
        bits.advance(entry.bits);
        Ok(entry.symbol)
    }

    /// Canonical decoding of a code longer than the table (RFC 1951 3.2.2):
    /// the codes of one length are consecutive numbers, so the first `n`
    /// bits are a code of length `n` exactly when they fall in that range.
    /// The table holds every code of at most `PRIMARY_BITS` bits, so a miss
    /// means the search can start one bit longer.
    #[cold]
    fn read_long(&self, bits: &mut Reader<'_>) -> Read<u16> {
        let (value, available) = bits.peek(self.width);
        // The first bits read are the code's most significant ones.
        let mut code =
            (value & (PRIMARY - 1)).reverse_bits() >> (usize::BITS - u32::from(PRIMARY_BITS));
        for length in PRIMARY_BITS + 1..=self.width {
            code = (code << 1) | ((value >> (length - 1)) & 1);
            let index = usize::from(length);
            if let Some(rank) = code.checked_sub(usize::from(self.first_code[index]))
                && rank < usize::from(self.counts[index])
            {
                if length > available {
                    return Err(Fault::End);
                }
                bits.advance(length);
                return Ok(self.symbols[usize::from(self.first_symbol[index]) + rank]);
            }
        }
        if available < self.width {
            return Err(Fault::End);
        }
        Err(syntax("invalid Huffman code", bits.offset).into())
    }
}

#[derive(Clone, Copy)]
struct Node {
    weight: u64,
    symbol: usize,
    children: Option<[usize; 2]>,
}

pub(super) struct Book<const N: usize> {
    pub(super) lengths: [u8; N],
    pub(super) codes: [u16; N],
}

impl<const N: usize> Book<N> {
    pub(super) fn from_lengths(lengths: [u8; N]) -> Self {
        let mut counts = [0_u16; 16];
        for &len in &lengths {
            if len != 0 {
                counts[usize::from(len)] += 1;
            }
        }
        let mut next = first_codes(&counts);
        let mut codes = [0; N];
        for (symbol, &len) in lengths.iter().enumerate() {
            if len != 0 {
                let code = &mut next[usize::from(len)];
                codes[symbol] = code.reverse_bits() >> (16 - len);
                *code += 1;
            }
        }
        Self { lengths, codes }
    }

    pub(super) fn build(frequencies: &[u32; N], max_bits: u8) -> Result<Self> {
        // Callers pass the RFC 1951 alphabets (at most 286 symbols) with
        // their maximum code lengths (15 or 7), which always fit.
        debug_assert!(N >= 2 && (1..=15).contains(&max_bits));
        let count = frequencies.iter().filter(|&&f| f != 0).count().max(2);
        debug_assert!(count <= 1 << max_bits);
        let mut nodes = with_capacity(count * usize::from(max_bits))?;
        for (symbol, &weight) in frequencies.iter().enumerate() {
            if weight != 0 {
                nodes.push(Node {
                    weight: u64::from(weight),
                    symbol,
                    children: None,
                });
            }
        }
        // A dummy symbol makes even empty/single-symbol alphabets complete.
        // Its frequency cannot affect the cost of any real input symbol.
        for (symbol, &frequency) in frequencies.iter().enumerate() {
            if nodes.len() >= 2 {
                break;
            }
            if frequency == 0 {
                nodes.push(Node {
                    weight: 0,
                    symbol,
                    children: None,
                });
            }
        }
        let mut leaves = with_capacity(count)?;
        leaves.extend(0..count);
        leaves.sort_unstable_by_key(|&id| (nodes[id].weight, id));
        let mut current = with_capacity(2 * count)?;
        current.extend_from_slice(&leaves);
        let mut next = with_capacity(2 * count)?;
        let mut packages = with_capacity(count)?;
        for _ in 1..max_bits {
            packages.clear();
            for pair in current.as_chunks::<2>().0 {
                let id = nodes.len();
                nodes.push(Node {
                    weight: nodes[pair[0]].weight + nodes[pair[1]].weight,
                    symbol: 0,
                    children: Some([pair[0], pair[1]]),
                });
                packages.push(id);
            }
            next.clear();
            let (mut leaf, mut package) = (0, 0);
            while leaf < leaves.len() || package < packages.len() {
                let take_leaf = package == packages.len()
                    || (leaf < leaves.len()
                        && (nodes[leaves[leaf]].weight, leaves[leaf])
                            <= (nodes[packages[package]].weight, packages[package]));
                if take_leaf {
                    next.push(leaves[leaf]);
                    leaf += 1;
                } else {
                    next.push(packages[package]);
                    package += 1;
                }
            }
            std::mem::swap(&mut current, &mut next);
        }
        // Each level keeps every leaf, so at least 2*count-2 items remain.
        let selected = &current[..2 * count - 2];
        let mut lengths = [0; N];
        // The stack has at most max_bits pending siblings plus selected roots.
        next.clear();
        reserve(&mut next, 2 * count + usize::from(max_bits))?;
        next.extend_from_slice(selected);
        while let Some(id) = next.pop() {
            let node = nodes[id];
            if let Some(children) = node.children {
                next.extend_from_slice(&children);
            } else {
                lengths[node.symbol] += 1;
            }
        }
        Ok(Self::from_lengths(lengths))
    }
}

#[cfg(test)]
mod tests {
    use super::{Alphabet, Book, Decoder};
    use crate::flate::bits::{Fault, Reader};
    use pagura_core::Meter;

    #[test]
    fn table_and_long_codes_decode_every_symbol_and_detect_truncation() {
        // A complete code with every length from 1 to 15: lengths up to 10
        // come from the table, longer ones from the canonical search.
        let mut lengths = [0_u8; 16];
        for (symbol, length) in lengths.iter_mut().enumerate().take(15) {
            *length = u8::try_from(symbol + 1).unwrap();
        }
        lengths[15] = 15;
        let mut work = Meter::new(usize::MAX, "test budget exceeded");
        let decoder = Decoder::new(&lengths, Alphabet::LiteralLength, &mut work, 0).unwrap();
        assert_eq!(work.used(), 1 << 10);
        let book = Book::from_lengths(lengths);
        for (symbol, &length) in lengths.iter().enumerate() {
            // The encoder's codes are stored in reading order, low bit first.
            let bytes = u32::from(book.codes[symbol]).to_le_bytes();
            let mut bits = Reader::new(&bytes, 0);
            let decoded = decoder.read(&mut bits).unwrap();
            assert_eq!(usize::from(decoded), symbol);
            assert_eq!(bits.offset, usize::from(length / 8), "{symbol}");
            let short = usize::from(length).div_ceil(8) - 1;
            if length > 8 && !length.is_multiple_of(8) {
                let mut bits = Reader::new(&bytes[..short], 0);
                assert!(
                    matches!(decoder.read(&mut bits), Err(Fault::End)),
                    "{symbol}"
                );
            }
        }
    }

    #[test]
    fn package_merge_matches_exhaustive_small_oracle() {
        // Independently enumerate every complete tree length assignment.
        for frequencies in [[1, 1, 1, 1], [1, 2, 4, 8], [1, 1, 1, 99], [10, 2, 3, 4]] {
            for max in 2..=4 {
                let book = Book::build(&frequencies, max).unwrap();
                let actual: u32 = frequencies
                    .iter()
                    .zip(book.lengths)
                    .map(|(&f, l)| f * u32::from(l))
                    .sum();
                let mut best = u32::MAX;
                for a in 1..=max {
                    for b in 1..=max {
                        for c in 1..=max {
                            for d in 1..=max {
                                let lengths = [a, b, c, d];
                                if lengths.iter().map(|&l| 1_u32 << (max - l)).sum::<u32>()
                                    == 1 << max
                                {
                                    let cost = frequencies
                                        .iter()
                                        .zip(lengths)
                                        .map(|(&f, l)| f * u32::from(l))
                                        .sum();
                                    best = best.min(cost);
                                }
                            }
                        }
                    }
                }
                assert_eq!(actual, best);
            }
        }
    }

    #[test]
    fn skewed_alphabets_are_complete_and_length_limited() {
        let mut frequencies = [0_u32; 286];
        frequencies[0] = 1;
        frequencies[1] = 1;
        for i in 2..30 {
            frequencies[i] = frequencies[i - 1] + frequencies[i - 2];
        }
        for max in [7, 15] {
            let book = Book::build(&frequencies, max).unwrap();
            assert!(book.lengths.iter().all(|&l| l <= max));
            assert!(book.lengths[..30].iter().all(|&l| l > 0));
            assert_eq!(
                book.lengths
                    .iter()
                    .filter(|&&l| l > 0)
                    .map(|&l| 1_u32 << (max - l))
                    .sum::<u32>(),
                1 << max
            );
            assert!(book.lengths[30..].iter().all(|&l| l == 0));
        }
        assert_eq!(
            Book::build(&[0_u32; 19], 7)
                .unwrap()
                .lengths
                .iter()
                .filter(|&&l| l > 0)
                .count(),
            2
        );
        let mut single = [0; 286];
        single[256] = 1;
        let book = Book::build(&single, 15).unwrap();
        assert_eq!(book.lengths[256], 1);
        assert_eq!(book.lengths.iter().filter(|&&l| l > 0).count(), 2);
    }
}
