//! Deterministic block selection and RFC 1950/1951 serialization.
use super::bits::Writer;
use super::huffman::Book;
use super::matcher::{Matcher, Token};
use super::tables::{
    BlockType, CODE_LENGTH_ORDER, CODE_LENGTH_SYMBOLS, DIST_BASE, DIST_EXTRA, DISTANCE_SYMBOLS,
    END_OF_BLOCK, FIRST_LENGTH, FIXED_DISTANCE_BITS, FIXED_LITERAL_SYMBOLS, LENGTH_BASE,
    LENGTH_EXTRA, LITERAL_SYMBOLS, MAX_CODE_BITS, MAX_CODE_LENGTH_BITS, MAX_STORED,
    MIN_CODE_LENGTH_CODES, REPEAT_PREVIOUS, REPEAT_ZERO, REPEAT_ZERO_LONG, fixed_literal_lengths,
};
use super::zlib::{self, Adler32};
use super::{Level, MIN_STREAM};
use crate::bounded::{BoundedBytes, ENCODE_BUDGET, encode_limit, with_capacity};
use pagura_core::{Limits, Result};

/// Input bytes per block: the longest stored block, so any block can be
/// stored.
const BLOCK: usize = MAX_STORED;
/// BFINAL and BTYPE (RFC 1951 3.2.3).
const BLOCK_HEADER_BITS: usize = 3;
/// HLIT, HDIST and HCLEN (RFC 1951 3.2.7).
const COUNT_FIELD_BITS: usize = 5 + 5 + 4;
/// Each stored code-length code length (RFC 1951 3.2.7).
const CODE_LENGTH_FIELD_BITS: usize = 3;
/// The smallest dynamic header; a fixed block no longer than this is never
/// beaten by a dynamic one.
const MIN_DYNAMIC_HEADER_BITS: usize =
    BLOCK_HEADER_BITS + COUNT_FIELD_BITS + CODE_LENGTH_FIELD_BITS * MIN_CODE_LENGTH_CODES;
/// LEN and NLEN of a stored block (RFC 1951 3.2.4).
const STORED_LENGTH_BITS: usize = 32;

/// Whether a block is the last one (BFINAL).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Last {
    Yes,
    No,
}

fn block_header(output: &mut Writer, block_type: BlockType, last: Last) -> Result<()> {
    let bfinal = u32::from(last == Last::Yes);
    output.put(bfinal | (u32::from(block_type as u8) << 1), 3)
}

fn put_symbol<const N: usize>(output: &mut Writer, book: &Book<N>, symbol: usize) -> Result<()> {
    output.put(u32::from(book.codes[symbol]), book.lengths[symbol])
}

/// One code-length symbol with its extra bits.
#[derive(Clone, Copy)]
struct Run {
    symbol: u8,
    value: u8,
    bits: u8,
}

struct Dynamic {
    literal: Book<LITERAL_SYMBOLS>,
    distance: Book<DISTANCE_SYMBOLS>,
    lengths: Book<CODE_LENGTH_SYMBOLS>,
    runs: Vec<Run>,
    literals: usize,
    distances: usize,
    count: usize,
    cost: usize,
}

/// The fixed code (RFC 1951 3.2.6), built once per stream.
struct Fixed {
    literal: Book<FIXED_LITERAL_SYMBOLS>,
    distance: Book<DISTANCE_SYMBOLS>,
}

impl Fixed {
    fn new() -> Self {
        Self {
            literal: Book::from_lengths(fixed_literal_lengths()),
            // All 32 fixed distance symbols participate in canonical
            // assignment; the used first 30 have the same five-bit codes in
            // this prefix.
            distance: Book::from_lengths([FIXED_DISTANCE_BITS; DISTANCE_SYMBOLS]),
        }
    }
}

fn length_index(length: usize) -> usize {
    LENGTH_BASE
        .iter()
        .rposition(|&base| base <= length)
        .unwrap_or(0)
}

fn distance_index(distance: usize) -> usize {
    DIST_BASE
        .iter()
        .rposition(|&base| base <= distance)
        .unwrap_or(0)
}

fn frequencies(tokens: &[Token]) -> ([u32; LITERAL_SYMBOLS], [u32; DISTANCE_SYMBOLS]) {
    let mut literals = [0; LITERAL_SYMBOLS];
    let mut distances = [0; DISTANCE_SYMBOLS];
    literals[usize::from(END_OF_BLOCK)] = 1;
    for &token in tokens {
        match token {
            Token::Literal(byte) => literals[usize::from(byte)] += 1,
            Token::Match { length, distance } => {
                literals[usize::from(FIRST_LENGTH) + length_index(usize::from(length))] += 1;
                distances[distance_index(usize::from(distance))] += 1;
            }
        }
    }
    (literals, distances)
}

fn payload_cost<const N: usize>(
    tokens: &[Token],
    literal: &Book<N>,
    distance: &Book<DISTANCE_SYMBOLS>,
) -> usize {
    let mut size = usize::from(literal.lengths[usize::from(END_OF_BLOCK)]);
    for &token in tokens {
        match token {
            Token::Literal(byte) => size += usize::from(literal.lengths[usize::from(byte)]),
            Token::Match {
                length,
                distance: offset,
            } => {
                let len = length_index(usize::from(length));
                let dist = distance_index(usize::from(offset));
                size += usize::from(literal.lengths[usize::from(FIRST_LENGTH) + len])
                    + usize::from(LENGTH_EXTRA[len])
                    + usize::from(distance.lengths[dist])
                    + usize::from(DIST_EXTRA[dist]);
            }
        }
    }
    size
}

/// Run-length codes for code lengths (RFC 1951 3.2.7): `REPEAT_PREVIOUS`
/// copies the previous length 3-6 times, `REPEAT_ZERO` writes 3-10 zeros and
/// `REPEAT_ZERO_LONG` 11-138.
fn length_runs(lengths: impl Iterator<Item = u8>) -> Result<Vec<Run>> {
    let mut values = with_capacity(LITERAL_SYMBOLS + DISTANCE_SYMBOLS)?;
    values.extend(lengths);
    let mut runs = with_capacity(values.len())?;
    let mut pos = 0;
    while pos < values.len() {
        let value = values[pos];
        let end = pos + values[pos..].iter().take_while(|&&v| v == value).count();
        let mut count = end - pos;
        if value != 0 {
            runs.push(Run {
                symbol: value,
                value: 0,
                bits: 0,
            });
            count -= 1;
            while count >= 3 {
                let repeat = count.min(6);
                runs.push(repeat_run(REPEAT_PREVIOUS, repeat - 3, 2));
                count -= repeat;
            }
        } else {
            while count >= 11 {
                let repeat = count.min(138);
                runs.push(repeat_run(REPEAT_ZERO_LONG, repeat - 11, 7));
                count -= repeat;
            }
            if count >= 3 {
                let repeat = count.min(10);
                runs.push(repeat_run(REPEAT_ZERO, repeat - 3, 3));
                count -= repeat;
            }
        }
        for _ in 0..count {
            runs.push(Run {
                symbol: value,
                value: 0,
                bits: 0,
            });
        }
        pos = end;
    }
    Ok(runs)
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "the largest repeat extra is 127"
)]
fn repeat_run(symbol: u8, value: usize, bits: u8) -> Run {
    Run {
        symbol,
        value: value as u8,
        bits,
    }
}

impl Dynamic {
    fn new(tokens: &[Token]) -> Result<Self> {
        let (lit_freq, dist_freq) = frequencies(tokens);
        let literal = Book::build(&lit_freq, MAX_CODE_BITS)?;
        let distance = Book::build(&dist_freq, MAX_CODE_BITS)?;
        let end_of_block = usize::from(END_OF_BLOCK);
        let literals = literal
            .lengths
            .iter()
            .rposition(|&l| l != 0)
            .unwrap_or(end_of_block)
            .max(end_of_block)
            + 1;
        let distances = distance.lengths.iter().rposition(|&l| l != 0).unwrap_or(0) + 1;
        let runs = length_runs(
            literal.lengths[..literals]
                .iter()
                .copied()
                .chain(distance.lengths[..distances].iter().copied()),
        )?;
        let mut frequencies = [0; CODE_LENGTH_SYMBOLS];
        for run in &runs {
            frequencies[usize::from(run.symbol)] += 1;
        }
        let lengths = Book::build(&frequencies, MAX_CODE_LENGTH_BITS)?;
        let count = CODE_LENGTH_ORDER
            .iter()
            .rposition(|&symbol| lengths.lengths[symbol] != 0)
            .unwrap_or(MIN_CODE_LENGTH_CODES - 1)
            .max(MIN_CODE_LENGTH_CODES - 1)
            + 1;
        let cost = BLOCK_HEADER_BITS
            + COUNT_FIELD_BITS
            + CODE_LENGTH_FIELD_BITS * count
            + payload_cost(tokens, &literal, &distance)
            + runs
                .iter()
                .map(|r| usize::from(lengths.lengths[usize::from(r.symbol)]) + usize::from(r.bits))
                .sum::<usize>();
        Ok(Self {
            literal,
            distance,
            lengths,
            runs,
            literals,
            distances,
            count,
            cost,
        })
    }

    fn header(&self, output: &mut Writer) -> Result<()> {
        output.put(small(self.literals - usize::from(FIRST_LENGTH)), 5)?;
        output.put(small(self.distances - 1), 5)?;
        output.put(small(self.count - MIN_CODE_LENGTH_CODES), 4)?;
        for &symbol in CODE_LENGTH_ORDER.iter().take(self.count) {
            output.put(u32::from(self.lengths.lengths[symbol]), 3)?;
        }
        for run in &self.runs {
            put_symbol(output, &self.lengths, usize::from(run.symbol))?;
            output.put(u32::from(run.value), run.bits)?;
        }
        Ok(())
    }
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "DEFLATE fields, block lengths and token differences are at most 65535"
)]
fn small(value: usize) -> u32 {
    value as u32
}

fn payload<const N: usize>(
    output: &mut Writer,
    tokens: &[Token],
    literal: &Book<N>,
    distance: &Book<DISTANCE_SYMBOLS>,
) -> Result<()> {
    for &token in tokens {
        match token {
            Token::Literal(byte) => put_symbol(output, literal, usize::from(byte))?,
            Token::Match {
                length,
                distance: offset,
            } => {
                let len = length_index(usize::from(length));
                let dist = distance_index(usize::from(offset));
                put_symbol(output, literal, usize::from(FIRST_LENGTH) + len)?;
                output.put(
                    small(usize::from(length) - LENGTH_BASE[len]),
                    LENGTH_EXTRA[len],
                )?;
                put_symbol(output, distance, dist)?;
                output.put(
                    small(usize::from(offset) - DIST_BASE[dist]),
                    DIST_EXTRA[dist],
                )?;
            }
        }
    }
    put_symbol(output, literal, usize::from(END_OF_BLOCK))
}

fn block(
    output: &mut Writer,
    fixed: &Fixed,
    bytes: &[u8],
    tokens: &[Token],
    last: Last,
) -> Result<()> {
    let fixed_cost = BLOCK_HEADER_BITS + payload_cost(tokens, &fixed.literal, &fixed.distance);
    let dynamic = if fixed_cost > MIN_DYNAMIC_HEADER_BITS {
        Some(Dynamic::new(tokens)?)
    } else {
        None
    };
    let padding = (8 - (usize::from(output.count) + BLOCK_HEADER_BITS) % 8) % 8;
    let stored_cost = BLOCK_HEADER_BITS + padding + STORED_LENGTH_BITS + bytes.len() * 8;
    let cost = |bits: usize| match last {
        Last::Yes => (usize::from(output.count) + bits).div_ceil(8) * 8,
        Last::No => bits,
    };
    let dynamic_cost = dynamic.as_ref().map_or(usize::MAX, |d| cost(d.cost));
    if cost(stored_cost) < cost(fixed_cost) && cost(stored_cost) <= dynamic_cost {
        block_header(output, BlockType::Stored, last)?;
        output.align()?;
        let length = u16::try_from(bytes.len()).map_err(|_| encode_limit())?;
        output.bytes.extend(&length.to_le_bytes())?;
        output.bytes.extend(&(!length).to_le_bytes())?;
        output.bytes.extend(bytes)?;
    } else if let Some(dynamic) = dynamic.filter(|_| dynamic_cost < cost(fixed_cost)) {
        block_header(output, BlockType::Dynamic, last)?;
        dynamic.header(output)?;
        payload(output, tokens, &dynamic.literal, &dynamic.distance)?;
    } else {
        block_header(output, BlockType::Fixed, last)?;
        payload(output, tokens, &fixed.literal, &fixed.distance)?;
    }
    Ok(())
}

pub(super) fn compress(data: &[u8], level: Level, limits: &Limits) -> Result<Vec<u8>> {
    if data.len() > limits.decoded_bytes || limits.stream_bytes < MIN_STREAM {
        return Err(encode_limit());
    }
    let mut output = Writer::new(BoundedBytes::new(limits.stream_bytes, ENCODE_BUDGET));
    output.bytes.extend(&zlib::header(level))?;
    let fixed = Fixed::new();
    let mut matcher = Matcher::new(data.len(), level)?;
    let mut tokens = with_capacity(data.len().min(BLOCK))?;
    if data.is_empty() {
        block(&mut output, &fixed, &[], &[], Last::Yes)?;
    }
    for (index, bytes) in data.chunks(BLOCK).enumerate() {
        let start = index * BLOCK;
        let end = start + bytes.len();
        matcher.tokenize(data, start, end, &mut tokens);
        let last = if end == data.len() {
            Last::Yes
        } else {
            Last::No
        };
        block(&mut output, &fixed, bytes, &tokens, last)?;
    }
    output.align()?;
    let mut adler = Adler32::new();
    adler.update(data);
    output.bytes.extend(&adler.finish().to_be_bytes())?;
    Ok(output.bytes.into_vec())
}
