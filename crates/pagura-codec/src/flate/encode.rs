//! Deterministic block selection and RFC 1950/1951 serialization.
use super::{
    DIST_BASE, DIST_EXTRA, LENGTH_BASE, LENGTH_EXTRA, Level,
    huffman::{Book, Workspace},
    matcher::{Matcher, Token, WINDOW, Window},
};
use crate::encoding::{Bytes, limit};
use pagura_core::{Error, Limits, Meter, Result};

#[cfg(test)]
mod tests;

const BLOCK: usize = 65535;
const ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

struct Bits {
    bytes: Bytes,
    pending: u32,
    count: u8,
}

impl Bits {
    fn put(&mut self, value: u32, width: u8) -> Result<()> {
        self.pending |= value << self.count;
        self.count += width;
        while self.count >= 8 {
            self.bytes.extend(&[self.pending.to_le_bytes()[0]])?;
            self.pending >>= 8;
            self.count -= 8;
        }
        Ok(())
    }

    fn align(&mut self) -> Result<()> {
        if self.count != 0 {
            self.bytes.extend(&[self.pending.to_le_bytes()[0]])?;
            self.pending = 0;
            self.count = 0;
        }
        Ok(())
    }

    fn symbol<const N: usize>(&mut self, book: &Book<N>, symbol: usize) -> Result<()> {
        self.put(u32::from(book.codes[symbol]), book.lengths[symbol])
    }
}

#[derive(Clone, Copy)]
struct Run {
    symbol: u8,
    value: u8,
    bits: u8,
}

struct Dynamic {
    literal: Book<286>,
    distance: Book<30>,
    lengths: Book<19>,
    literals: usize,
    distances: usize,
    count: usize,
    cost: usize,
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

fn frequencies(tokens: &[Token]) -> ([u32; 286], [u32; 30]) {
    let mut literals = [0; 286];
    let mut distances = [0; 30];
    literals[256] = 1;
    for token in tokens {
        if token.distance == 0 {
            literals[usize::from(token.value)] += 1;
        } else {
            literals[257 + length_index(usize::from(token.value))] += 1;
            distances[distance_index(usize::from(token.distance))] += 1;
        }
    }
    (literals, distances)
}

fn payload_cost<const N: usize>(tokens: &[Token], literal: &Book<N>, distance: &Book<30>) -> usize {
    let mut size = usize::from(literal.lengths[256]);
    for token in tokens {
        if token.distance == 0 {
            size += usize::from(literal.lengths[usize::from(token.value)]);
        } else {
            let len = length_index(usize::from(token.value));
            let dist = distance_index(usize::from(token.distance));
            size += usize::from(literal.lengths[257 + len])
                + usize::from(LENGTH_EXTRA[len])
                + usize::from(distance.lengths[dist])
                + usize::from(DIST_EXTRA[dist]);
        }
    }
    size
}

fn length_runs(lengths: impl Iterator<Item = u8>, runs: &mut Vec<Run>) -> Result<()> {
    let mut values = [0; 316];
    let mut length = 0;
    for (index, value) in lengths.enumerate() {
        *values.get_mut(index).ok_or_else(|| limit(0))? = value;
        length = index + 1;
    }
    let values = &values[..length];
    runs.clear();
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
                runs.push(repeat_run(16, repeat - 3, 2));
                count -= repeat;
            }
        } else {
            while count >= 11 {
                let repeat = count.min(138);
                runs.push(repeat_run(18, repeat - 11, 7));
                count -= repeat;
            }
            if count >= 3 {
                let repeat = count.min(10);
                runs.push(repeat_run(17, repeat - 3, 3));
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
    Ok(())
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
    fn new(tokens: &[Token], workspace: &mut Workspace, runs: &mut Vec<Run>) -> Result<Self> {
        let (lit_freq, dist_freq) = frequencies(tokens);
        let literal = Book::build(&lit_freq, 15, workspace)?;
        let distance = Book::build(&dist_freq, 15, workspace)?;
        let literals = literal
            .lengths
            .iter()
            .rposition(|&l| l != 0)
            .unwrap_or(256)
            .max(256)
            + 1;
        let distances = distance.lengths.iter().rposition(|&l| l != 0).unwrap_or(0) + 1;
        length_runs(
            literal.lengths[..literals]
                .iter()
                .copied()
                .chain(distance.lengths[..distances].iter().copied()),
            runs,
        )?;
        let mut frequencies = [0; 19];
        for run in runs.iter() {
            frequencies[usize::from(run.symbol)] += 1;
        }
        let lengths = Book::build(&frequencies, 7, workspace)?;
        let count = ORDER
            .iter()
            .rposition(|&symbol| lengths.lengths[symbol] != 0)
            .unwrap_or(3)
            .max(3)
            + 1;
        let cost = 3
            + 14
            + 3 * count
            + payload_cost(tokens, &literal, &distance)
            + runs
                .iter()
                .map(|r| usize::from(lengths.lengths[usize::from(r.symbol)]) + usize::from(r.bits))
                .sum::<usize>();
        Ok(Self {
            literal,
            distance,
            lengths,
            literals,
            distances,
            count,
            cost,
        })
    }

    fn header(&self, output: &mut Bits, runs: &[Run]) -> Result<()> {
        output.put(small(self.literals - 257), 5)?;
        output.put(small(self.distances - 1), 5)?;
        output.put(small(self.count - 4), 4)?;
        for &symbol in ORDER.iter().take(self.count) {
            output.put(u32::from(self.lengths.lengths[symbol]), 3)?;
        }
        for run in runs {
            output.symbol(&self.lengths, usize::from(run.symbol))?;
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
    output: &mut Bits,
    tokens: &[Token],
    literal: &Book<N>,
    distance: &Book<30>,
) -> Result<()> {
    for token in tokens {
        if token.distance == 0 {
            output.symbol(literal, usize::from(token.value))?;
        } else {
            let len = length_index(usize::from(token.value));
            let dist = distance_index(usize::from(token.distance));
            output.symbol(literal, 257 + len)?;
            output.put(
                small(usize::from(token.value) - LENGTH_BASE[len]),
                LENGTH_EXTRA[len],
            )?;
            output.symbol(distance, dist)?;
            output.put(
                small(usize::from(token.distance) - DIST_BASE[dist]),
                DIST_EXTRA[dist],
            )?;
        }
    }
    output.symbol(literal, 256)
}

fn fixed() -> (Book<288>, Book<30>) {
    let mut lengths = [8; 288];
    lengths[144..256].fill(9);
    lengths[256..280].fill(7);
    // All 32 fixed distance symbols participate in canonical assignment;
    // the used first 30 have the same five-bit codes in this prefix.
    (Book::from_lengths(lengths), Book::from_lengths([5; 30]))
}

fn block(
    output: &mut Bits,
    bytes: &[u8],
    tokens: &[Token],
    final_block: bool,
    workspace: &mut Workspace,
    runs: &mut Vec<Run>,
) -> Result<()> {
    let (literal, distance) = fixed();
    let fixed_cost = 3 + payload_cost(tokens, &literal, &distance);
    let dynamic = if fixed_cost > 29 {
        Some(Dynamic::new(tokens, workspace, runs)?)
    } else {
        None
    };
    let stored_cost = 3 + (8 - (usize::from(output.count) + 3) % 8) % 8 + 32 + bytes.len() * 8;
    let cost = |bits: usize| {
        if final_block {
            (usize::from(output.count) + bits).div_ceil(8) * 8
        } else {
            bits
        }
    };
    let dynamic_cost = dynamic.as_ref().map_or(usize::MAX, |d| cost(d.cost));
    if cost(stored_cost) < cost(fixed_cost) && cost(stored_cost) <= dynamic_cost {
        output.put(u32::from(final_block), 3)?;
        output.align()?;
        let length = u16::try_from(bytes.len()).map_err(|_| limit(0))?;
        output.bytes.extend(&length.to_le_bytes())?;
        output.bytes.extend(&(!length).to_le_bytes())?;
        output.bytes.extend(bytes)?;
    } else if let Some(dynamic) = dynamic.filter(|_| dynamic_cost < cost(fixed_cost)) {
        output.put(4 + u32::from(final_block), 3)?;
        dynamic.header(output, runs)?;
        payload(output, tokens, &dynamic.literal, &dynamic.distance)?;
    } else {
        output.put(2 + u32::from(final_block), 3)?;
        payload(output, tokens, &literal, &distance)?;
    }
    Ok(())
}

/// One canonical block result; output is borrowed until the next mutation.
pub(crate) enum Encoded<'a> {
    NeedInput,
    Bytes(&'a [u8]),
    Complete,
}

/// Bounded input history and output, shared by full-buffer and PNG collectors.
pub(crate) struct Compressor {
    total: usize,
    received: usize,
    processed: usize,
    origin: usize,
    input: Vec<u8>,
    matcher: Matcher,
    tokens: Vec<Token>,
    workspace: Workspace,
    runs: Vec<Run>,
    output: Bits,
    adler: super::Adler32,
    done: bool,
}

impl Compressor {
    pub(crate) fn new(total: usize, level: Level, bytes: &mut Meter) -> Result<Self> {
        let mut input = Vec::new();
        bytes.reserve_capacity(&mut input, total.min(WINDOW + BLOCK + 2))?;
        let mut tokens = Vec::new();
        bytes.reserve_capacity(&mut tokens, total.min(BLOCK))?;
        let mut runs = Vec::new();
        bytes.reserve_capacity(&mut runs, 316)?;
        let mut output = Bits {
            bytes: Bytes::reserved(total.min(BLOCK) + 16, bytes)?,
            pending: 0,
            count: 0,
        };
        output.bytes.extend(if level == Level::Fast {
            &[0x78, 0x01]
        } else {
            &[0x78, 0x9c]
        })?;
        Ok(Self {
            total,
            received: 0,
            processed: 0,
            origin: 0,
            input,
            matcher: Matcher::new(total, level, bytes)?,
            tokens,
            workspace: Workspace::new(bytes)?,
            runs,
            output,
            adler: super::Adler32::new(),
            done: false,
        })
    }

    pub(crate) fn output(&self) -> &[u8] {
        &self.output.bytes.data
    }

    pub(crate) fn finished(&self) -> bool {
        self.done
    }

    fn block_end(&self) -> usize {
        self.processed.saturating_add(BLOCK).min(self.total)
    }

    fn wanted(&self) -> usize {
        self.block_end().saturating_add(2).min(self.total)
    }

    /// Accepts only what the next canonical block needs, including hash lookahead.
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Result<usize> {
        if bytes.len() > self.total.saturating_sub(self.received) {
            return Err(Error::invalid_argument(
                "compression input exceeds declared length",
            ));
        }
        let count = bytes.len().min(self.wanted().saturating_sub(self.received));
        self.input.extend_from_slice(&bytes[..count]);
        self.adler.update(&bytes[..count]);
        self.received += count;
        Ok(count)
    }

    /// One synchronous canonical-block step, with no allocation after admission.
    pub(crate) fn advance(&mut self) -> Result<Encoded<'_>> {
        if self.done {
            return Ok(Encoded::Complete);
        }
        if self.received < self.wanted() {
            return Ok(Encoded::NeedInput);
        }
        // The first block retains its zlib header; later blocks reuse storage.
        if self.processed != 0 {
            self.output.bytes.data.clear();
        }
        let end = self.block_end();
        self.matcher.tokenize(
            Window {
                bytes: &self.input,
                origin: self.origin,
            },
            self.processed,
            end,
            &mut self.tokens,
        );
        let final_block = end == self.total;
        block(
            &mut self.output,
            &self.input[self.processed - self.origin..end - self.origin],
            &self.tokens,
            final_block,
            &mut self.workspace,
            &mut self.runs,
        )?;
        if final_block {
            self.output.align()?;
            self.output
                .bytes
                .extend(&self.adler.value().to_be_bytes())?;
            self.done = true;
        }
        let origin = end.saturating_sub(WINDOW);
        let discarded = origin - self.origin;
        self.input.copy_within(discarded.., 0);
        self.input.truncate(self.input.len() - discarded);
        self.origin = origin;
        self.processed = end;
        Ok(Encoded::Bytes(&self.output.bytes.data))
    }
}

pub(super) fn compress(data: &[u8], level: Level, limits: &Limits) -> Result<Vec<u8>> {
    if data.len() > limits.decoded_bytes || limits.stream_bytes < 8 {
        return Err(limit(data.len()));
    }
    // The compatibility collector retains its exact final-output quota. The
    // streaming PNG admission separately checks a conservative output bound.
    let mut memory = Meter::new(2 * 1024 * 1024, "compression scratch budget exceeded");
    let mut encoder = Compressor::new(data.len(), level, &mut memory)?;
    let mut output = Bytes::new(limits.stream_bytes);
    let mut received = 0;
    loop {
        received += encoder.push(&data[received..])?;
        match encoder.advance()? {
            Encoded::Bytes(bytes) => output.extend(bytes)?,
            Encoded::Complete => return Ok(output.data),
            Encoded::NeedInput => {
                return Err(Error::invalid_argument("incomplete compression input"));
            }
        }
    }
}
