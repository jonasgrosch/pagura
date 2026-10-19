//! Zlib/DEFLATE, RFC 1950 sections 2.2/2.3 and RFC 1951 section 3.2.
//! [`compress`] chooses stored, fixed or dynamic blocks with bounded LZ77 work.
//! Canonical Huffman codes are expanded into direct lookup tables. Backrefs
//! read predictor-free output directly; predictors retain a bounded circular
//! window of pre-predictor bytes (at most 32 KiB).
use crate::{Predictor, output::Output, repair, syntax, tags, trailing};
use pagura_core::{Diagnostics, Error, Result};

mod adler;
mod encode;
mod huffman;
mod matcher;

/// Compression effort. Both modes emit deterministic RFC 1950 zlib streams.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Level {
    /// Short hash-chain searches and greedy matches.
    Fast,
    /// Deeper searches with one-byte lazy matching.
    #[default]
    Default,
}

/// Compresses bytes to zlib (RFC 1950 2.2) containing DEFLATE (RFC 1951 3.2).
///
/// Uses at most a 32 KiB history and 65,535-byte blocks. Search depth and match
/// length are bounded, so work is linear in input length with level-dependent
/// constants. Scratch storage is bounded independently of large input sizes.
/// `Limits::decoded_bytes` bounds input and `Limits::stream_bytes` bounds output.
/// Empty input produces a complete stream; no preset dictionary is used.
///
/// # Errors
///
/// Returns a positioned `Limit` error before exceeding either byte budget or
/// when bounded scratch/output storage cannot be allocated.
///
/// # Examples
///
/// ```
/// use pagura_codec::flate::{compress, Level};
/// use pagura_core::Limits;
/// let zlib = compress(b"PDF stream bytes", Level::Default, &Limits::default())?;
/// assert!(zlib.len() >= 8);
/// # Ok::<(), pagura_core::Error>(())
/// ```
pub fn compress(data: &[u8], level: Level, limits: &pagura_core::Limits) -> Result<Vec<u8>> {
    encode::compress(data, level, limits)
}

// RFC 1951 3.2.5, length and distance tables.
const LENGTH_BASE: [usize; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [usize; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

#[derive(Debug)]
enum Fault {
    End,
    Invalid(Error),
}
impl From<Error> for Fault {
    fn from(error: Error) -> Self {
        Self::Invalid(error)
    }
}
type InflateResult<T> = std::result::Result<T, Fault>;

pub(crate) fn decode(
    input: &[u8],
    cap: usize,
    predictor: Predictor,
    diagnostics: &mut Diagnostics,
) -> Result<Vec<u8>> {
    // RFC 1951 stored-block padding can mimic CM=8. Prefer a valid zlib
    // header, otherwise recognize a structurally valid raw stored block.
    // Never retry raw decoding after accepting a zlib header.
    let candidate = input.first().is_some_and(|byte| byte & 15 == 8);
    let wrapped = candidate && (valid_zlib_header(input) || !raw_stored_header(input));
    if !wrapped {
        repair(
            diagnostics,
            tags::RAW_DEFLATE,
            "FlateDecode data lacks the zlib wrapper; decoded as raw DEFLATE",
            0,
        )?;
    }
    inflate(input, cap, predictor, diagnostics, wrapped)
}

pub(crate) fn raw(
    input: &[u8],
    cap: usize,
    predictor: Predictor,
    diagnostics: &mut Diagnostics,
) -> Result<Vec<u8>> {
    inflate(input, cap, predictor, diagnostics, false)
}

fn inflate(
    input: &[u8],
    cap: usize,
    predictor: Predictor,
    diagnostics: &mut Diagnostics,
    wrapped: bool,
) -> Result<Vec<u8>> {
    if wrapped && input.len() < 2 {
        repair(
            diagnostics,
            tags::TRUNCATED,
            "zlib header truncated",
            input.len(),
        )?;
        return Output::new(cap, predictor)?.finish(input.len());
    }
    let (start, window) = if wrapped { header(input)? } else { (0, 32768) };
    let mut bits = Bits {
        input,
        offset: start,
        bit: 0,
    };
    let output = Output::new(cap, predictor)?;
    let history = if predictor.kind == crate::PredictorKind::None {
        None
    } else {
        Some(History::new(output.history_capacity().min(window))?)
    };
    let mut inflater = Inflater {
        output,
        history,
        window,
        adler_a: 1,
        adler_b: 0,
        adler_pending: 0,
    };
    match inflater.blocks(&mut bits) {
        Ok(()) => {
            bits.align();
            if wrapped {
                let checksum_offset = bits.offset;
                let Some(checksum) = input.get(checksum_offset..).and_then(|tail| tail.get(..4))
                else {
                    repair(
                        diagnostics,
                        tags::TRUNCATED,
                        "zlib checksum truncated",
                        input.len(),
                    )?;
                    return inflater.output.finish(input.len());
                };
                let expected = checksum
                    .iter()
                    .fold(0_u32, |value, &byte| (value << 8) | u32::from(byte));
                inflater.adler_a %= 65521;
                inflater.adler_b %= 65521;
                if expected != (inflater.adler_b << 16) | inflater.adler_a {
                    repair(
                        diagnostics,
                        tags::CHECKSUM_MISMATCH,
                        "zlib Adler-32 mismatch",
                        checksum_offset,
                    )?;
                }
                bits.offset += 4;
            }
            trailing(input, bits.offset, false, diagnostics)?;
            inflater.output.finish(bits.offset)
        }
        Err(Fault::End) => {
            repair(
                diagnostics,
                tags::TRUNCATED,
                "DEFLATE input truncated",
                input.len(),
            )?;
            inflater.output.finish(input.len())
        }
        Err(Fault::Invalid(error)) => Err(error),
    }
}

fn valid_zlib_header(input: &[u8]) -> bool {
    matches!(input.get(..2), Some([cmf, flg])
        if cmf & 15 == 8 && cmf >> 4 <= 7
            && (u16::from(*cmf) * 256 + u16::from(*flg)) % 31 == 0)
}

fn raw_stored_header(input: &[u8]) -> bool {
    matches!(input.get(1..5), Some([lo, hi, nlo, nhi])
        if lo ^ nlo == 255 && hi ^ nhi == 255)
}

fn header(input: &[u8]) -> Result<(usize, usize)> {
    let Some((&cmf, tail)) = input.split_first() else {
        return Err(syntax("zlib header missing", 0));
    };
    let Some(&flg) = tail.first() else {
        return Err(syntax("zlib header truncated", input.len()));
    };
    if cmf >> 4 > 7 || (u16::from(cmf) * 256 + u16::from(flg)) % 31 != 0 {
        return Err(syntax("invalid zlib header", 0));
    }
    if flg & 32 != 0 {
        // RFC 1950 2.3 forbids FDICT in an enclosing format without preset
        // dictionaries; ISO 32000-2 7.4.4 defines none for FlateDecode.
        return Err(syntax("zlib preset dictionary forbidden by PDF", 1));
    }
    Ok((2, 1 << ((cmf >> 4) + 8)))
}

struct Bits<'a> {
    input: &'a [u8],
    offset: usize,
    bit: u8,
}
impl Bits<'_> {
    fn peek(&self, width: u8) -> (usize, u8) {
        if let Some(bytes) = self
            .input
            .get(self.offset..)
            .and_then(|tail| tail.first_chunk::<4>())
        {
            let value = u32::from_le_bytes(*bytes) >> self.bit;
            return (
                usize::try_from(value & ((1 << width) - 1)).unwrap_or(0),
                width,
            );
        }
        let mut value = 0_usize;
        let mut available = 0;
        for index in 0..3 {
            let Some(&byte) = self
                .offset
                .checked_add(index)
                .and_then(|i| self.input.get(i))
            else {
                break;
            };
            value |= usize::from(byte) << (index * 8);
            available += 8;
        }
        (
            value >> self.bit & ((1 << width) - 1),
            (available - self.bit).min(width),
        )
    }
    fn read(&mut self, width: u8) -> InflateResult<usize> {
        let (value, available) = self.peek(width);
        if available < width {
            return Err(Fault::End);
        }
        self.advance(width);
        Ok(value)
    }
    fn advance(&mut self, width: u8) {
        let total = self.bit + width;
        self.offset += usize::from(total / 8);
        self.bit = total % 8;
    }
    fn align(&mut self) {
        if self.bit != 0 {
            self.offset += 1;
            self.bit = 0;
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Entry {
    symbol: u16,
    bits: u8,
}
struct Huffman {
    entries: Vec<Entry>,
    width: u8,
}
impl Huffman {
    fn new(lengths: &[u8], allow_single: bool, allow_empty: bool, offset: usize) -> Result<Self> {
        let mut counts = [0_usize; 16];
        for &length in lengths {
            let slot = counts
                .get_mut(usize::from(length))
                .ok_or_else(|| syntax("invalid Huffman code length", offset))?;
            *slot += 1;
        }
        let width = lengths.iter().copied().max().unwrap_or(0);
        if width == 0 {
            if allow_empty {
                return Ok(Self {
                    entries: Vec::new(),
                    width: 0,
                });
            }
            return Err(syntax("empty Huffman alphabet", offset));
        }
        let mut remaining = 1_isize;
        for &count in counts.iter().skip(1) {
            remaining = remaining * 2
                - isize::try_from(count)
                    .map_err(|_| syntax("Huffman alphabet too large", offset))?;
            if remaining < 0 {
                return Err(syntax("oversubscribed Huffman alphabet", offset));
            }
        }
        // RFC 1951 3.2.7 explicitly allows a one-bit single distance code.
        // General incomplete-tree rejection is decoder policy: 3.2.1/3.2.2
        // require prefix/canonical codes but do not explicitly require a
        // complete tree. The same one-symbol exception permits EOB-only blocks.
        if remaining != 0 && !(allow_single && width == 1 && counts[1] == 1) {
            return Err(syntax("incomplete Huffman alphabet", offset));
        }
        let mut next = [0_usize; 16];
        // Zero lengths do not participate in canonical code assignment.
        next[1] = 0;
        for bits in 2..=15 {
            next[bits] = (next[bits - 1] + counts[bits - 1]) << 1;
        }
        let size = 1_usize << width;
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(size)
            .map_err(|_| crate::limit(offset))?;
        entries.resize(size, Entry::default());
        for (symbol, &length) in lengths.iter().enumerate() {
            if length == 0 {
                continue;
            }
            let slot = next
                .get_mut(usize::from(length))
                .ok_or_else(|| syntax("invalid Huffman length", offset))?;
            let reversed = slot.reverse_bits() >> (usize::BITS - u32::from(length));
            *slot += 1;
            let entry = Entry {
                symbol: u16::try_from(symbol)
                    .map_err(|_| syntax("Huffman symbol too large", offset))?,
                bits: length,
            };
            for index in (reversed..size).step_by(1 << length) {
                if let Some(target) = entries.get_mut(index) {
                    *target = entry;
                }
            }
        }
        Ok(Self { entries, width })
    }
    fn read(&self, bits: &mut Bits<'_>) -> InflateResult<u16> {
        if self.width == 0 {
            return Err(syntax("distance code in empty alphabet", bits.offset).into());
        }
        let (index, available) = bits.peek(self.width);
        let entry = self
            .entries
            .get(index)
            .copied()
            .ok_or_else(|| syntax("invalid Huffman code", bits.offset))?;
        if entry.bits == 0 {
            if available < self.width {
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
}

// Allocated only when predictor reversal would change the history bytes.
struct History {
    bytes: Vec<u8>,
    cursor: usize,
    available: usize,
}

impl History {
    fn new(size: usize) -> Result<Self> {
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(size).map_err(|_| crate::limit(0))?;
        bytes.resize(size, 0);
        Ok(Self {
            bytes,
            cursor: 0,
            available: 0,
        })
    }

    fn write(&mut self, mut source: &[u8], offset: usize) -> Result<()> {
        if self.bytes.is_empty() {
            return Ok(());
        }
        while !source.is_empty() {
            let count = source.len().min(self.bytes.len() - self.cursor);
            let (head, tail) = source.split_at(count);
            self.bytes
                .get_mut(self.cursor..self.cursor + count)
                .ok_or_else(|| syntax("invalid DEFLATE history range", offset))?
                .copy_from_slice(head);
            self.cursor += count;
            if self.cursor == self.bytes.len() {
                self.cursor = 0;
            }
            self.available = (self.available + count).min(self.bytes.len());
            source = tail;
        }
        Ok(())
    }

    fn seed(&self, distance: usize, target: &mut [u8], offset: usize) -> Result<()> {
        if distance > self.available {
            return Err(syntax("DEFLATE distance exceeds history", offset));
        }
        let start = if self.cursor >= distance {
            self.cursor - distance
        } else {
            self.bytes.len() - (distance - self.cursor)
        };
        let first = target.len().min(self.bytes.len() - start);
        let (head, tail) = target.split_at_mut(first);
        head.copy_from_slice(
            self.bytes
                .get(start..start + first)
                .ok_or_else(|| syntax("invalid DEFLATE history range", offset))?,
        );
        tail.copy_from_slice(
            self.bytes
                .get(..tail.len())
                .ok_or_else(|| syntax("invalid DEFLATE history range", offset))?,
        );
        Ok(())
    }
}

struct Inflater {
    output: Output,
    history: Option<History>,
    window: usize,
    adler_a: u32,
    adler_b: u32,
    adler_pending: usize,
}
impl Inflater {
    fn emit(&mut self, bytes: &[u8], offset: usize) -> Result<()> {
        self.output.write(bytes, offset)?;
        adler::update(
            bytes,
            &mut self.adler_a,
            &mut self.adler_b,
            &mut self.adler_pending,
        );
        if let Some(history) = &mut self.history {
            history.write(bytes, offset)?;
        }
        Ok(())
    }
    fn blocks(&mut self, bits: &mut Bits<'_>) -> InflateResult<()> {
        loop {
            let final_block = bits.read(1)? != 0;
            match bits.read(2)? {
                0 => self.stored(bits)?,
                1 => {
                    let mut lengths = [8; 288];
                    lengths[144..256].fill(9);
                    lengths[256..280].fill(7);
                    let literal = Huffman::new(&lengths, false, false, bits.offset)?;
                    let distance = Huffman::new(&[5; 32], false, false, bits.offset)?;
                    self.compressed(bits, &literal, &distance)?;
                }
                2 => {
                    let (literal, distance) = dynamic(bits)?;
                    self.compressed(bits, &literal, &distance)?;
                }
                _ => return Err(syntax("reserved DEFLATE block type", bits.offset).into()),
            }
            if final_block {
                return Ok(());
            }
        }
    }
    fn stored(&mut self, bits: &mut Bits<'_>) -> InflateResult<()> {
        bits.align();
        let length = bits.read(16)?;
        if bits.read(16)? != (length ^ 65535) {
            return Err(syntax("DEFLATE stored length mismatch", bits.offset).into());
        }
        let remaining = bits.input.len().saturating_sub(bits.offset);
        let count = remaining.min(length);
        let end = bits
            .offset
            .checked_add(count)
            .ok_or_else(|| crate::limit(bits.offset))?;
        let bytes = bits.input.get(bits.offset..end).ok_or(Fault::End)?;
        self.emit(bytes, bits.offset)?;
        bits.offset = end;
        if count < length {
            return Err(Fault::End);
        }
        Ok(())
    }
    fn compressed(
        &mut self,
        bits: &mut Bits<'_>,
        literal: &Huffman,
        distance: &Huffman,
    ) -> InflateResult<()> {
        // Without prediction, decoded bytes already are the DEFLATE history.
        // Accumulate their checksum once per block rather than once per symbol.
        if self.history.is_none() {
            let start = self.output.buffer().len();
            let result = self.compressed_inner::<true>(bits, literal, distance);
            adler::update(
                &self.output.buffer()[start..],
                &mut self.adler_a,
                &mut self.adler_b,
                &mut self.adler_pending,
            );
            return result;
        }
        self.compressed_inner::<false>(bits, literal, distance)
    }

    fn compressed_inner<const RAW: bool>(
        &mut self,
        bits: &mut Bits<'_>,
        literal: &Huffman,
        distance: &Huffman,
    ) -> InflateResult<()> {
        loop {
            match literal.read(bits)? {
                symbol @ 0..=255 => {
                    let byte =
                        u8::try_from(symbol).map_err(|_| syntax("invalid literal", bits.offset))?;
                    if RAW {
                        self.output.literal(byte, bits.offset)?;
                    } else {
                        self.emit(&[byte], bits.offset)?;
                    }
                }
                256 => return Ok(()),
                symbol @ 257..=285 => {
                    let index = usize::from(symbol - 257);
                    let base = LENGTH_BASE
                        .get(index)
                        .copied()
                        .ok_or_else(|| syntax("invalid DEFLATE length", bits.offset))?;
                    let extra = LENGTH_EXTRA
                        .get(index)
                        .copied()
                        .ok_or_else(|| syntax("invalid DEFLATE length", bits.offset))?;
                    let length = base + bits.read(extra)?;
                    // RFC 1951 section 3.2.5 assigns code 284 lengths 227..257;
                    // length 258 has its own code (285).
                    if symbol == 284 && length == 258 {
                        return Err(syntax(
                            "DEFLATE length code exceeds assigned range",
                            bits.offset,
                        )
                        .into());
                    }
                    let index = usize::from(distance.read(bits)?);
                    let base = DIST_BASE
                        .get(index)
                        .copied()
                        .ok_or_else(|| syntax("reserved DEFLATE distance", bits.offset))?;
                    let extra = DIST_EXTRA
                        .get(index)
                        .copied()
                        .ok_or_else(|| syntax("reserved DEFLATE distance", bits.offset))?;
                    let distance = base + bits.read(extra)?;
                    if distance > self.window {
                        return Err(syntax("DEFLATE distance exceeds history", bits.offset).into());
                    }
                    if RAW {
                        self.output.copy(distance, length, bits.offset)?;
                    } else {
                        self.copy(distance, length, bits.offset)?;
                    }
                }
                _ => return Err(syntax("reserved DEFLATE literal", bits.offset).into()),
            }
        }
    }
    fn copy(&mut self, distance: usize, length: usize, offset: usize) -> Result<()> {
        let mut scratch = [0; 258];
        let seed = distance.min(length);
        let target = scratch
            .get_mut(..seed)
            .ok_or_else(|| syntax("invalid DEFLATE copy length", offset))?;
        let Some(history) = &self.history else {
            return Err(syntax("DEFLATE distance exceeds history", offset));
        };
        history.seed(distance, target, offset)?;
        self.output.check(length, offset)?;
        let mut filled = seed;
        while filled < length {
            let count = filled.min(length - filled);
            scratch.copy_within(..count, filled);
            filled += count;
        }
        self.emit(
            scratch
                .get(..length)
                .ok_or_else(|| syntax("invalid DEFLATE copy length", offset))?,
            offset,
        )
    }
}

fn dynamic(bits: &mut Bits<'_>) -> InflateResult<(Huffman, Huffman)> {
    let literals = bits.read(5)? + 257;
    if literals > 286 {
        return Err(syntax("invalid DEFLATE literal count", bits.offset).into());
    }
    let distances = bits.read(5)? + 1;
    let count = bits.read(4)? + 4;
    let order = [
        16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
    ];
    let mut code_lengths = [0; 19];
    for &symbol in order.iter().take(count) {
        let value =
            u8::try_from(bits.read(3)?).map_err(|_| syntax("invalid code length", bits.offset))?;
        if let Some(slot) = code_lengths.get_mut(symbol) {
            *slot = value;
        }
    }
    let codes = Huffman::new(&code_lengths, false, false, bits.offset)?;
    let total = literals + distances;
    let mut lengths = [0; 318];
    let mut cursor = 0;
    while cursor < total {
        let symbol = codes.read(bits)?;
        let (value, repeat) = match symbol {
            0..=15 => (
                u8::try_from(symbol).map_err(|_| syntax("invalid code length", bits.offset))?,
                1,
            ),
            16 => (
                lengths
                    .get(cursor.checked_sub(1).ok_or_else(|| {
                        syntax("Huffman repeat without previous length", bits.offset)
                    })?)
                    .copied()
                    .ok_or_else(|| syntax("Huffman repeat without previous length", bits.offset))?,
                bits.read(2)? + 3,
            ),
            17 => (0, bits.read(3)? + 3),
            18 => (0, bits.read(7)? + 11),
            _ => return Err(syntax("invalid code-length symbol", bits.offset).into()),
        };
        let end = cursor + repeat;
        if end > total {
            return Err(syntax("Huffman repeat exceeds alphabet", bits.offset).into());
        }
        if let Some(target) = lengths.get_mut(cursor..end) {
            target.fill(value);
        }
        cursor = end;
    }
    if lengths.get(256).copied().unwrap_or(0) == 0 {
        return Err(syntax("DEFLATE end code missing from alphabet", bits.offset).into());
    }
    let literal = Huffman::new(
        lengths
            .get(..literals)
            .ok_or_else(|| syntax("invalid literal alphabet", bits.offset))?,
        true,
        false,
        bits.offset,
    )?;
    let distance = Huffman::new(
        lengths
            .get(literals..total)
            .ok_or_else(|| syntax("invalid distance alphabet", bits.offset))?,
        true,
        true,
        bits.offset,
    )?;
    Ok((literal, distance))
}
