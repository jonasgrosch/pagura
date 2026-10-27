//! Zlib/DEFLATE, RFC 1950 sections 2.2/2.3 and RFC 1951 section 3.2.
//! [`compress`] chooses stored, fixed or dynamic blocks with bounded LZ77 work.
//!
//! Decoding resolves each Huffman code with one lookup, or two for codes
//! longer than the primary index, in tables of combined entries. While
//! enough input and output room remain, a fast loop decodes symbols without
//! per-symbol limit checks; it leaves every symbol those margins do not cover
//! (block ends, invalid codes, distances beyond the history, the last bytes of
//! input, the output cap) to a careful step that checks it exactly. Without a
//! predictor, decoded bytes go straight to the output, which is also the
//! history; with one, a window keeps the last 32 KiB of pre-predictor bytes.
//! The entry layout and flag bits, the 11- and 8-bit primary tables and the
//! bit refill follow the design libdeflate describes (ADR 0006).
use crate::{Predictor, PredictorKind, output::Output, repair, syntax, tags, trailing};
use pagura_core::{Diagnostics, Error, Result};

mod adler;
mod bits;
mod encode;
mod huffman;
mod matcher;
mod table;
#[cfg(test)]
mod tests;

use adler::Adler32;
use bits::Bits;
use table::{
    CODE_284, DistanceTable, END_OF_BLOCK, EXCEPTIONAL, LINK, LITERAL, LitlenTable, PrecodeTable,
    base_plus_extra, code_length, total, value,
};

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

/// The longest match (RFC 1951 3.2.5).
const MAX_MATCH: usize = 258;
/// Bytes a fast-loop match may write past its end: copies move 16 at a time.
const COPY_SLACK: usize = 15;
/// Output room the fast loop needs before each symbol.
const FAST_OUTPUT: usize = MAX_MATCH + COPY_SLACK;
/// Input the fast loop needs before each symbol: two 8-byte refills, one
/// before a symbol and one after a match.
const FAST_INPUT: usize = 16;
/// History a predictor window retains, the largest DEFLATE distance.
const HISTORY: usize = 32768;

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

/// `size` is the expected output length, an allocation hint only (0 for
/// none).
pub(crate) fn decode(
    input: &[u8],
    cap: usize,
    predictor: Predictor,
    size: usize,
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
    inflate(input, (cap, size), predictor, diagnostics, wrapped)
}

pub(crate) fn raw(
    input: &[u8],
    cap: usize,
    predictor: Predictor,
    diagnostics: &mut Diagnostics,
) -> Result<Vec<u8>> {
    inflate(input, (cap, 0), predictor, diagnostics, false)
}

fn inflate(
    input: &[u8],
    (cap, size): (usize, usize),
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
        return Output::new(cap, predictor)?.finish(input.len(), diagnostics);
    }
    let (start, window) = if wrapped {
        header(input)?
    } else {
        (0, HISTORY)
    };
    let mut bits = Bits::new(input, start);
    let output = Output::new(cap, predictor)?;
    let mut inflater = Inflater::new(output, predictor, cap, window, (input.len(), size));
    match inflater.blocks(&mut bits) {
        Ok(()) => {
            bits.align();
            let mut end = bits.aligned_offset();
            if wrapped {
                let Some(checksum) = input.get(end..).and_then(|tail| tail.first_chunk::<4>())
                else {
                    repair(
                        diagnostics,
                        tags::TRUNCATED,
                        "zlib checksum truncated",
                        input.len(),
                    )?;
                    return inflater.sink.finish(input.len(), diagnostics);
                };
                if u32::from_be_bytes(*checksum) != inflater.adler.value() {
                    repair(
                        diagnostics,
                        tags::CHECKSUM_MISMATCH,
                        "zlib Adler-32 mismatch",
                        end,
                    )?;
                }
                end += 4;
            }
            trailing(input, end, false, diagnostics)?;
            inflater.sink.finish(end, diagnostics)
        }
        Err(Fault::End) => {
            repair(
                diagnostics,
                tags::TRUNCATED,
                "DEFLATE input truncated",
                input.len(),
            )?;
            inflater.sink.finish(input.len(), diagnostics)
        }
        Err(Fault::Invalid(error)) => {
            crate::damaged(error, diagnostics, || inflater.sink.complete())
        }
    }
}

/// The leading output of zlib data, inflated a block at a time until
/// `enough` accepts it: the bytes a whole decode would start with. `None`
/// when the data lacks a valid zlib header, when any block before that point
/// is truncated or invalid, when `enough` abandons the prefix, or when the
/// stream ends first; a caller then decodes the whole stream, which reports
/// those conditions as before.
pub(crate) fn prefix(
    input: &[u8],
    (cap, size): (usize, usize),
    enough: &mut dyn FnMut(&[u8]) -> crate::Prefix,
) -> Option<Vec<u8>> {
    if !valid_zlib_header(input) {
        return None;
    }
    let (start, window) = header(input).ok()?;
    let mut bits = Bits::new(input, start);
    let none = Predictor {
        kind: PredictorKind::None,
        colors: 1,
        bits_per_component: 8,
        columns: 1,
    };
    let output = Output::new(cap, none).ok()?;
    let mut inflater = Inflater::new(output, none, cap, window, (input.len(), size));
    loop {
        let final_block = bits.read(1).ok()? != 0;
        inflater.block(&mut bits).ok()?;
        match enough(&inflater.sink.bytes[..inflater.sink.len]) {
            crate::Prefix::Enough => {
                let mut strict = Diagnostics::new(pagura_core::Recovery::Strict);
                return inflater.sink.finish(bits.offset(), &mut strict).ok();
            }
            crate::Prefix::Abandon => return None,
            crate::Prefix::More if final_block => return None,
            crate::Prefix::More => {}
        }
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

/// Pre-predictor bytes. Without a predictor they are the output, and the
/// storage's capacity grows exactly as [`Output`] would have reserved it, so
/// callers that account retained capacity see the same figures. With one,
/// they are a window of history ahead of the predictor.
struct Sink {
    /// Zero-initialized storage; `bytes[..len]` are decoded.
    bytes: Vec<u8>,
    len: usize,
    /// `bytes[..done]` are checksummed and, with a predictor, in `output`.
    done: usize,
    cap: usize,
    /// Without a predictor: the capacity `Output` would hold by now. The
    /// storage may be one doubling ahead while the fast loop runs.
    reserved: usize,
    output: Output,
    predicted: bool,
    /// Set once a fast run met an invalid predictor tag; the careful step
    /// then reproduces the error at its symbol.
    careful_only: bool,
}

impl Sink {
    /// Without a predictor, storage for an expected `size` is allocated at
    /// once, twice over plus a margin so that the doubling bookkeeping never
    /// outgrows it for that size; [`Self::finish`] trims it to the capacity
    /// the bookkeeping reached, as without the hint. Untouched storage
    /// stays unused.
    fn new(output: Output, predicted: bool, cap: usize, (input, size): (usize, usize)) -> Self {
        let mut bytes = Vec::new();
        if !predicted && size > 0 {
            let _ = bytes.try_reserve_exact(size.saturating_mul(2).saturating_add(1024).min(cap));
        }
        if predicted {
            // A small first window; it grows to its bound only for large streams.
            // A power of two, so its doublings end exactly at the bound.
            let size = input
                .saturating_mul(4)
                .clamp(4096, 2 * HISTORY)
                .next_power_of_two();
            if bytes.try_reserve_exact(size).is_ok() {
                bytes.resize(size, 0);
            }
        }
        Self {
            bytes,
            len: 0,
            done: 0,
            cap,
            reserved: 0,
            output,
            predicted,
            careful_only: false,
        }
    }

    /// `Output::reserve`'s growth rule, then storage to match it.
    fn reserve(&mut self, count: usize, offset: usize) -> Result<()> {
        let required = self
            .len
            .checked_add(count)
            .filter(|&n| n <= self.cap)
            .ok_or_else(|| crate::limit(offset))?;
        if required > self.reserved {
            self.reserved = required
                .max(self.reserved.saturating_mul(2).max(64))
                .min(self.cap);
            if self.bytes.capacity() < self.reserved {
                self.bytes
                    .try_reserve_exact(self.reserved - self.bytes.len())
                    .map_err(|_| crate::limit(offset))?;
            }
        }
        self.initialize(required);
        Ok(())
    }

    /// Zero-fills storage up to at least `end` (within capacity), a chunk at
    /// a time so unused capacity stays untouched.
    fn initialize(&mut self, end: usize) {
        if self.bytes.len() < end {
            let target = end.max(self.len + HISTORY).min(self.bytes.capacity());
            self.bytes.resize(target.max(end), 0);
        }
    }

    /// Storage the fast loop may write: `bytes[..limit]`, if that leaves it
    /// room for at least one symbol.
    fn fast_limit(&mut self) -> Option<usize> {
        if self.careful_only {
            return None;
        }
        if self.predicted {
            return self.window_limit();
        }
        // Below 258 reserved bytes a single match can outgrow a doubling, so
        // the careful step reproduces each reservation there.
        let room = self.len.checked_add(FAST_OUTPUT)?;
        if self.reserved < MAX_MATCH || self.cap - self.len < MAX_MATCH {
            return None;
        }
        if room > self.bytes.capacity() {
            // One doubling ahead of `reserved`; settled once the run ends.
            let ahead = self.reserved.saturating_mul(2).min(self.cap);
            if ahead < room
                || self
                    .bytes
                    .try_reserve_exact(ahead - self.bytes.len())
                    .is_err()
            {
                return None;
            }
        }
        // Capacity never exceeds the cap, so neither does the run.
        self.initialize(room);
        Some(self.bytes.len())
    }

    /// Without a predictor, every doubling a fast run crossed (RFC limits
    /// each symbol to 258 bytes, at most one doubling at 258 or more).
    fn settle(&mut self) {
        while self.reserved < self.len {
            self.reserved = self.reserved.saturating_mul(2).min(self.cap);
        }
    }

    /// With a predictor: slides the window once the history and room for a
    /// symbol no longer fit, and bounds the run by the cap.
    fn window_limit(&mut self) -> Option<usize> {
        if self.bytes.len() - self.len < FAST_OUTPUT + MAX_MATCH {
            self.slide();
        }
        // Pre-predictor bytes never undercount output bytes, so a run within
        // the remaining cap cannot exceed it.
        let room = self.output.room().min(self.bytes.len() - self.len);
        let limit = self.len + room;
        (room >= FAST_OUTPUT).then_some(limit)
    }

    /// Makes room in the predictor window, keeping the last 32 KiB. Every
    /// byte before `len` is already in the output.
    fn slide(&mut self) {
        let grown = (2 * self.bytes.len()).clamp(4096, 4 * HISTORY);
        if grown > self.bytes.len()
            && self
                .bytes
                .try_reserve_exact(grown - self.bytes.len())
                .is_ok()
        {
            self.bytes.resize(grown, 0);
        } else if self.len > HISTORY {
            self.bytes.copy_within(self.len - HISTORY..self.len, 0);
            self.len = HISTORY;
            self.done = HISTORY;
        }
    }

    /// Checksums decoded bytes and hands them to the predictor.
    fn flush(&mut self, adler: &mut Adler32, offset: usize) -> Result<()> {
        let Some(bytes) = self.bytes.get(self.done..self.len) else {
            return Ok(());
        };
        adler.update(bytes);
        if self.predicted {
            self.output.write(bytes, offset)?;
        }
        self.done = self.len;
        Ok(())
    }

    fn literal(&mut self, byte: u8, offset: usize, adler: &mut Adler32) -> Result<()> {
        if self.predicted {
            if self.len == self.bytes.len() {
                self.slide();
            }
            let slot = self
                .bytes
                .get_mut(self.len)
                .ok_or_else(|| crate::limit(offset))?;
            *slot = byte;
            self.len += 1;
            return self.flush(adler, offset);
        }
        self.reserve(1, offset)?;
        if let Some(slot) = self.bytes.get_mut(self.len) {
            *slot = byte;
        }
        self.len += 1;
        Ok(())
    }

    /// Copies a back-reference already checked against the history.
    fn copy(
        &mut self,
        distance: usize,
        length: usize,
        offset: usize,
        adler: &mut Adler32,
    ) -> Result<()> {
        if self.predicted {
            self.output.check(length, offset)?;
            if self.bytes.len() - self.len < length {
                self.slide();
            }
        } else {
            self.reserve(length, offset)?;
        }
        let start = self.len - distance;
        let end = self.len + length;
        if end > self.bytes.len() {
            return Err(crate::limit(offset));
        }
        let mut filled = self.len;
        while filled < end {
            let count = (filled - start).min(end - filled);
            self.bytes.copy_within(start..start + count, filled);
            filled += count;
        }
        self.len = end;
        if self.predicted {
            self.flush(adler, offset)?;
        }
        Ok(())
    }

    /// Stored-block bytes (RFC 1951 3.2.4), checked against the cap as one.
    fn stored(&mut self, mut bytes: &[u8], offset: usize, adler: &mut Adler32) -> Result<()> {
        if !self.predicted {
            self.reserve(bytes.len(), offset)?;
            let target = self
                .bytes
                .get_mut(self.len..self.len + bytes.len())
                .ok_or_else(|| crate::limit(offset))?;
            target.copy_from_slice(bytes);
            self.len += bytes.len();
            return Ok(());
        }
        self.output.check(bytes.len(), offset)?;
        while !bytes.is_empty() {
            if self.len == self.bytes.len() {
                self.slide();
            }
            let count = bytes.len().min(self.bytes.len() - self.len);
            if count == 0 {
                return Err(crate::limit(offset));
            }
            let (head, tail) = bytes.split_at(count);
            if let Some(target) = self.bytes.get_mut(self.len..self.len + count) {
                target.copy_from_slice(head);
            }
            self.len += count;
            self.flush(adler, offset)?;
            bytes = tail;
        }
        Ok(())
    }

    fn finish(self, offset: usize, diagnostics: &mut Diagnostics) -> Result<Vec<u8>> {
        if self.predicted {
            return self.output.finish(offset, diagnostics);
        }
        Ok(self.unpredicted())
    }

    /// The whole rows decoded before damaged data (`filter.corrupt`): bytes
    /// still in the window go to the predictor, which keeps the rows it
    /// accepts.
    fn complete(mut self) -> Vec<u8> {
        if !self.predicted {
            return self.unpredicted();
        }
        if let Some(bytes) = self.bytes.get(self.done..self.len) {
            // A predictor tag it refuses ends the rows; that is the damage.
            let _ = self.output.write(bytes, 0);
        }
        self.output.complete()
    }

    fn unpredicted(self) -> Vec<u8> {
        let mut bytes = self.bytes;
        bytes.truncate(self.len);
        if bytes.capacity() > self.reserved {
            bytes.shrink_to(self.reserved);
        }
        bytes
    }
}

struct Inflater {
    sink: Sink,
    adler: Adler32,
    window: usize,
    litlen: LitlenTable,
    distance: DistanceTable,
    /// Whether the tables hold the fixed codes (RFC 1951 3.2.6).
    fixed: bool,
}

impl Inflater {
    fn new(
        output: Output,
        predictor: Predictor,
        cap: usize,
        window: usize,
        (input, size): (usize, usize),
    ) -> Self {
        let predicted = predictor.kind != PredictorKind::None;
        Self {
            sink: Sink::new(output, predicted, cap, (input, size)),
            adler: Adler32::new(),
            window,
            litlen: LitlenTable::new(),
            distance: DistanceTable::new(),
            fixed: false,
        }
    }

    fn blocks(&mut self, bits: &mut Bits<'_>) -> InflateResult<()> {
        loop {
            let final_block = bits.read(1)? != 0;
            self.block(bits)?;
            if final_block {
                return Ok(());
            }
        }
    }

    /// One block after its final-block bit.
    fn block(&mut self, bits: &mut Bits<'_>) -> InflateResult<()> {
        match bits.read(2)? {
            0 => self.stored(bits),
            1 => {
                if !self.fixed {
                    let mut lengths = [8; 288];
                    lengths[144..256].fill(9);
                    lengths[256..280].fill(7);
                    table::litlen(&mut self.litlen, &lengths, false, bits.offset())?;
                    table::distance(&mut self.distance, &[5; 32], false, bits.offset())?;
                    self.fixed = true;
                }
                self.compressed(bits)
            }
            2 => {
                self.fixed = false;
                dynamic(bits, &mut self.litlen, &mut self.distance)?;
                self.compressed(bits)
            }
            _ => Err(syntax("reserved DEFLATE block type", bits.offset()).into()),
        }
    }

    fn stored(&mut self, bits: &mut Bits<'_>) -> InflateResult<()> {
        bits.align();
        let length = bits.read(16)? as usize;
        if bits.read(16)? as usize != (length ^ 65535) {
            return Err(syntax("DEFLATE stored length mismatch", bits.offset()).into());
        }
        let offset = bits.aligned_offset();
        let remaining = bits.input.len().saturating_sub(offset);
        let count = remaining.min(length);
        let end = offset
            .checked_add(count)
            .ok_or_else(|| crate::limit(offset))?;
        let bytes = bits.input.get(offset..end).ok_or(Fault::End)?;
        self.sink.stored(bytes, offset, &mut self.adler)?;
        bits.restart(end);
        if count < length {
            return Err(Fault::End);
        }
        if !self.sink.predicted {
            self.sink.flush(&mut self.adler, end)?;
        }
        Ok(())
    }

    fn compressed(&mut self, bits: &mut Bits<'_>) -> InflateResult<()> {
        loop {
            if let Some(limit) = self.sink.fast_limit() {
                let resume = (bits.next, bits.buffer, bits.count, self.sink.len);
                if let Some(storage) = self.sink.bytes.get_mut(..limit) {
                    fast(
                        bits,
                        &self.litlen,
                        &self.distance,
                        storage,
                        &mut self.sink.len,
                        self.window,
                    );
                }
                if self.sink.predicted {
                    if self
                        .sink
                        .output
                        .valid_tags(&self.sink.bytes[resume.3..self.sink.len])
                    {
                        self.sink.flush(&mut self.adler, bits.offset())?;
                    } else {
                        // Decode the run again, a symbol at a time, so the
                        // invalid tag fails at its symbol's offset.
                        (bits.next, bits.buffer, bits.count, self.sink.len) = resume;
                        self.sink.careful_only = true;
                    }
                } else {
                    self.sink.settle();
                    if self.sink.len - self.sink.done >= 4 * HISTORY {
                        self.sink.flush(&mut self.adler, bits.offset())?;
                    }
                }
            }
            if self.careful(bits)? {
                break;
            }
        }
        if !self.sink.predicted {
            self.sink.flush(&mut self.adler, bits.offset())?;
        }
        Ok(())
    }

    /// Decodes one symbol with every check in the format's order. Returns
    /// whether it ended the block.
    fn careful(&mut self, bits: &mut Bits<'_>) -> InflateResult<bool> {
        let entry = self.litlen.decode(bits)?;
        if entry & LITERAL != 0 {
            let byte = entry.to_le_bytes()[2];
            self.sink.literal(byte, bits.offset(), &mut self.adler)?;
            return Ok(false);
        }
        if entry & END_OF_BLOCK != 0 {
            return Ok(true);
        }
        if entry & EXCEPTIONAL != 0 {
            return Err(syntax("reserved DEFLATE literal", bits.offset()).into());
        }
        let length = value(entry) as usize + bits.read(total(entry) - code_length(entry))? as usize;
        // RFC 1951 section 3.2.5 assigns code 284 lengths 227..257;
        // length 258 has its own code (285).
        if entry & CODE_284 != 0 && length == MAX_MATCH {
            return Err(syntax("DEFLATE length code exceeds assigned range", bits.offset()).into());
        }
        if self.distance.is_empty() {
            return Err(syntax("distance code in empty alphabet", bits.offset()).into());
        }
        let entry = self.distance.decode(bits)?;
        if entry & EXCEPTIONAL != 0 {
            return Err(syntax("reserved DEFLATE distance", bits.offset()).into());
        }
        let distance =
            value(entry) as usize + bits.read(total(entry) - code_length(entry))? as usize;
        if distance > self.window || distance > self.sink.len {
            return Err(syntax("DEFLATE distance exceeds history", bits.offset()).into());
        }
        self.sink
            .copy(distance, length, bits.offset(), &mut self.adler)?;
        Ok(false)
    }
}

/// Decodes symbols into `out` from `*at` while at least [`FAST_INPUT`] input
/// bytes and [`FAST_OUTPUT`] output bytes remain. Every symbol it takes is
/// valid, within the history and within `out`, which the caller bounds by
/// the cap; it stops before anything else, leaving `bits` at that symbol.
///
/// Each iteration starts just after a refill, with at least 56 bits counted
/// (all 64 buffer bits hold input) and the primary entry for them already
/// loaded: up to three literals (15 bits each), or a length and distance with
/// their extra bits (48 bits). The next entry is loaded before the copy, so
/// the lookup overlaps the copy. Bit counts below never underflow by this
/// accounting, so the arithmetic wraps rather than checks.
#[inline(never)]
fn fast(
    bits: &mut Bits<'_>,
    litlen: &LitlenTable,
    distance: &DistanceTable,
    out: &mut [u8],
    at: &mut usize,
    window: usize,
) {
    let input = bits.input;
    let (Some(input_end), Some(output_end)) = (
        input.len().checked_sub(FAST_INPUT),
        out.len().checked_sub(FAST_OUTPUT),
    ) else {
        return;
    };
    let (mut buffer, mut count, mut next, mut position) = (bits.buffer, bits.count, bits.next, *at);
    // One refill: `next` stays at most `input_end + 7`, so 8 bytes remain.
    // `count` is below 64, so `count ^ 63` is `63 - count`.
    let refill = |buffer: &mut u64, count: &mut u32, next: &mut usize| {
        if let Some(word) = input.get(*next..).and_then(|tail| tail.first_chunk::<8>()) {
            *buffer |= u64::from_le_bytes(*word).wrapping_shl(*count);
            *next += ((*count ^ 63) >> 3) as usize;
            *count |= bits::REFILLED;
        }
    };
    if next > input_end || position > output_end {
        return;
    }
    refill(&mut buffer, &mut count, &mut next);
    let mut entry = litlen.primary(buffer);
    loop {
        if entry & LITERAL != 0 {
            // `position` is at most `output_end`, so three bytes are free.
            let Some(slots) = out.get_mut(position..position + 3) else {
                break;
            };
            for slot in slots {
                *slot = entry.to_le_bytes()[2];
                position += 1;
                buffer = buffer.wrapping_shr(total(entry));
                count = count.wrapping_sub(total(entry));
                entry = litlen.primary(buffer);
                if entry & LITERAL == 0 {
                    break;
                }
            }
        } else if entry & EXCEPTIONAL == 0 {
            let (start_buffer, start_count) = (buffer, count);
            let length = base_plus_extra(entry, buffer);
            buffer = buffer.wrapping_shr(total(entry));
            count = count.wrapping_sub(total(entry));
            let mut link = distance.primary(buffer);
            if link & LINK != 0 {
                link = distance.secondary(link, buffer);
            }
            let span = base_plus_extra(link, buffer);
            if link & EXCEPTIONAL != 0
                || span > position.min(window)
                || (length == MAX_MATCH && entry & CODE_284 != 0)
            {
                (buffer, count) = (start_buffer, start_count);
                break;
            }
            buffer = buffer.wrapping_shr(total(link));
            count = count.wrapping_sub(total(link));
            // The refill at the top loaded all 64 bits with input and this
            // match took at most 48, so the next primary index (11 bits) is
            // already in the buffer; the refill below only ORs in the same
            // bits again, and the lookup need not wait for it.
            entry = litlen.primary(buffer);
            refill(&mut buffer, &mut count, &mut next);
            copy_match(out, position, span, length);
            position += length;
        } else if entry & LINK != 0 {
            // A code longer than the primary index; nothing is consumed yet.
            entry = litlen.secondary(entry, buffer);
            continue;
        } else {
            break;
        }
        if next > input_end || position > output_end {
            break;
        }
        refill(&mut buffer, &mut count, &mut next);
    }
    (bits.buffer, bits.count, bits.next, *at) = (buffer, count, next, position);
}

/// Copies a back-reference of `length` bytes from `distance` (1..=`at`)
/// back, writing up to [`COPY_SLACK`] bytes beyond it, which `out` holds:
/// the fast loop starts each symbol with [`FAST_OUTPUT`] bytes of room.
#[expect(
    clippy::inline_always,
    reason = "the copy shares the decode loop's registers"
)]
#[inline(always)]
fn copy_match(out: &mut [u8], at: usize, distance: usize, length: usize) {
    let end = at + length;
    let mut to = at;
    if distance >= 16 {
        // Each 16-byte source lies before its destination, written already.
        while to < end {
            out.copy_within(to - distance..to - distance + 16, to);
            to += 16;
        }
    } else if distance >= 8 {
        while to < end {
            out.copy_within(to - distance..to - distance + 8, to);
            to += 8;
        }
    } else if distance == 1 {
        let fill = [out[at - 1]; 16];
        while to < end {
            out[to..to + 16].copy_from_slice(&fill);
            to += 16;
        }
    } else {
        // Shorter periods: the first eight bytes one at a time, then
        // eight-byte copies at a stride that is a multiple of the period.
        for index in at..at + 8 {
            out[index] = out[index - distance];
        }
        let stride = [8, 8, 8, 6, 8, 5, 6, 7][distance];
        to += stride;
        while to < end {
            out.copy_within(to - stride..to - stride + 8, to);
            to += stride;
        }
    }
}

fn dynamic(
    bits: &mut Bits<'_>,
    litlen: &mut LitlenTable,
    distance: &mut DistanceTable,
) -> InflateResult<()> {
    let literals = bits.read(5)? as usize + 257;
    if literals > 286 {
        return Err(syntax("invalid DEFLATE literal count", bits.offset()).into());
    }
    let distances = bits.read(5)? as usize + 1;
    let count = bits.read(4)? as usize + 4;
    let order = [
        16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
    ];
    let mut code_lengths = [0; 19];
    for &symbol in order.iter().take(count) {
        let value = u8::try_from(bits.read(3)?)
            .map_err(|_| syntax("invalid code length", bits.offset()))?;
        if let Some(slot) = code_lengths.get_mut(symbol) {
            *slot = value;
        }
    }
    let mut codes = PrecodeTable::new();
    table::precode(&mut codes, &code_lengths, bits.offset())?;
    let total = literals + distances;
    let mut lengths = [0; 318];
    let mut cursor = 0;
    while cursor < total {
        let symbol = value(codes.decode(bits)?);
        let (value, repeat) = match symbol {
            0..=15 => (
                u8::try_from(symbol).map_err(|_| syntax("invalid code length", bits.offset()))?,
                1,
            ),
            16 => (
                lengths
                    .get(cursor.checked_sub(1).ok_or_else(|| {
                        syntax("Huffman repeat without previous length", bits.offset())
                    })?)
                    .copied()
                    .ok_or_else(|| {
                        syntax("Huffman repeat without previous length", bits.offset())
                    })?,
                bits.read(2)? as usize + 3,
            ),
            17 => (0, bits.read(3)? as usize + 3),
            18 => (0, bits.read(7)? as usize + 11),
            _ => return Err(syntax("invalid code-length symbol", bits.offset()).into()),
        };
        let end = cursor + repeat;
        if end > total {
            return Err(syntax("Huffman repeat exceeds alphabet", bits.offset()).into());
        }
        if let Some(target) = lengths.get_mut(cursor..end) {
            target.fill(value);
        }
        cursor = end;
    }
    if lengths.get(256).copied().unwrap_or(0) == 0 {
        return Err(syntax("DEFLATE end code missing from alphabet", bits.offset()).into());
    }
    table::litlen(
        litlen,
        lengths
            .get(..literals)
            .ok_or_else(|| syntax("invalid literal alphabet", bits.offset()))?,
        true,
        bits.offset(),
    )?;
    table::distance(
        distance,
        lengths
            .get(literals..total)
            .ok_or_else(|| syntax("invalid distance alphabet", bits.offset()))?,
        true,
        bits.offset(),
    )?;
    Ok(())
}
