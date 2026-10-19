//! DEFLATE decoding (RFC 1951 3.2) inside optional zlib framing (RFC 1950 2.2).
use super::bits::{Fault, Read, Reader};
use super::huffman::{Alphabet, Decoder};
use super::tables::{
    BlockType, CODE_LENGTH_ORDER, CODE_LENGTH_SYMBOLS, DIST_BASE, DIST_EXTRA, END_OF_BLOCK,
    FIRST_LENGTH, FIXED_DISTANCE_BITS, FIXED_DISTANCE_SYMBOLS, LAST_LENGTH, LENGTH_BASE,
    LENGTH_EXTRA, LITERAL_SYMBOLS, MAX_MATCH, MAX_STORED, MIN_CODE_LENGTH_CODES, REPEAT_PREVIOUS,
    REPEAT_ZERO, REPEAT_ZERO_LONG, WINDOW, fixed_literal_lengths,
};
use super::zlib::{self, Adler32};
use crate::report::{Trailer, repair, syntax, trailing};
use crate::{Predictor, PredictorKind, output::Output, repairs};
use pagura_core::{Diagnostics, Meter, Result};

/// Whether DEFLATE data is wrapped in zlib framing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Framing {
    Zlib,
    Raw,
}

pub(crate) fn decode(
    input: &[u8],
    cap: usize,
    predictor: Predictor,
    work: &mut Meter,
    diagnostics: &mut Diagnostics,
) -> Result<Vec<u8>> {
    // RFC 1951 stored-block padding can mimic CM=8. Prefer a valid zlib
    // header, otherwise recognize a structurally valid raw stored block.
    // Never retry raw decoding after accepting a zlib header.
    let candidate = input.first().is_some_and(|&cmf| zlib::is_deflate(cmf));
    let framing = if candidate && (valid_zlib_header(input) || !raw_stored_header(input)) {
        Framing::Zlib
    } else {
        repair(
            diagnostics,
            repairs::RAW_DEFLATE,
            "FlateDecode data lacks the zlib wrapper; decoded as raw DEFLATE",
            0,
        )?;
        Framing::Raw
    };
    inflate(input, cap, predictor, framing, work, diagnostics)
}

pub(crate) fn raw(
    input: &[u8],
    cap: usize,
    predictor: Predictor,
    work: &mut Meter,
    diagnostics: &mut Diagnostics,
) -> Result<Vec<u8>> {
    inflate(input, cap, predictor, Framing::Raw, work, diagnostics)
}

fn inflate(
    input: &[u8],
    cap: usize,
    predictor: Predictor,
    framing: Framing,
    work: &mut Meter,
    diagnostics: &mut Diagnostics,
) -> Result<Vec<u8>> {
    if framing == Framing::Zlib && input.len() < zlib::HEADER {
        repair(
            diagnostics,
            repairs::TRUNCATED,
            "zlib header truncated",
            input.len(),
        )?;
        return Output::new(cap, predictor)?.finish(input.len());
    }
    let (start, window) = match framing {
        Framing::Zlib => header(input)?,
        Framing::Raw => (0, WINDOW),
    };
    let mut bits = Reader::new(input, start);
    let output = Output::new(cap, predictor)?;
    let history = if predictor.kind() == PredictorKind::None {
        None
    } else {
        Some(History::new(output.history_capacity().min(window))?)
    };
    let mut inflater = Inflater {
        output,
        history,
        window,
        adler: Adler32::new(),
    };
    match inflater.blocks(&mut bits, work) {
        Ok(()) => {
            bits.align();
            if framing == Framing::Zlib {
                let checksum_offset = bits.offset;
                let Some(checksum) = input
                    .get(checksum_offset..)
                    .and_then(|tail| tail.first_chunk::<{ zlib::TRAILER }>())
                else {
                    repair(
                        diagnostics,
                        repairs::TRUNCATED,
                        "zlib checksum truncated",
                        input.len(),
                    )?;
                    return inflater.output.finish(input.len());
                };
                if u32::from_be_bytes(*checksum) != inflater.adler.finish() {
                    repair(
                        diagnostics,
                        repairs::CHECKSUM_MISMATCH,
                        "zlib Adler-32 mismatch",
                        checksum_offset,
                    )?;
                }
                bits.offset += zlib::TRAILER;
            }
            trailing(input, bits.offset, Trailer::Nothing, diagnostics)?;
            inflater.output.finish(bits.offset)
        }
        Err(Fault::End) => {
            repair(
                diagnostics,
                repairs::TRUNCATED,
                "DEFLATE input truncated",
                input.len(),
            )?;
            inflater.output.finish(input.len())
        }
        Err(Fault::Invalid(error)) => Err(error),
    }
}

fn valid_zlib_header(input: &[u8]) -> bool {
    matches!(input.get(..zlib::HEADER), Some(&[cmf, flg]) if zlib::window(cmf, flg).is_some())
}

fn raw_stored_header(input: &[u8]) -> bool {
    matches!(input.get(1..5), Some([lo, hi, nlo, nhi])
        if lo ^ nlo == 255 && hi ^ nhi == 255)
}

/// Where the DEFLATE data starts and the window the header declares.
fn header(input: &[u8]) -> Result<(usize, usize)> {
    let Some((&cmf, tail)) = input.split_first() else {
        return Err(syntax("zlib header missing", 0));
    };
    let Some(&flg) = tail.first() else {
        return Err(syntax("zlib header truncated", input.len()));
    };
    let window = zlib::window(cmf, flg).ok_or_else(|| syntax("invalid zlib header", 0))?;
    if flg & zlib::PRESET_DICTIONARY != 0 {
        // RFC 1950 2.3 forbids FDICT in an enclosing format without preset
        // dictionaries; 7.4.4 defines none for FlateDecode.
        return Err(syntax("zlib preset dictionary forbidden by PDF", 1));
    }
    Ok((zlib::HEADER, window))
}

// Allocated only when predictor reversal would change the history bytes.
struct History {
    bytes: Vec<u8>,
    cursor: usize,
    available: usize,
}

impl History {
    fn new(size: usize) -> Result<Self> {
        Ok(Self {
            bytes: crate::bounded::filled(size, 0)?,
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
    adler: Adler32,
}

impl Inflater {
    fn emit(&mut self, bytes: &[u8], offset: usize) -> Result<()> {
        self.output.write(bytes, offset)?;
        self.adler.update(bytes);
        if let Some(history) = &mut self.history {
            history.write(bytes, offset)?;
        }
        Ok(())
    }

    fn blocks(&mut self, bits: &mut Reader<'_>, work: &mut Meter) -> Read<()> {
        // The fixed code's tables, built once for every fixed block.
        let mut fixed = None;
        loop {
            let final_block = bits.read(1)? != 0;
            match BlockType::from_bits(bits.read(2)?) {
                Some(BlockType::Stored) => self.stored(bits)?,
                Some(BlockType::Fixed) => {
                    if fixed.is_none() {
                        fixed = Some(fixed_tables(work, bits.offset)?);
                    }
                    if let Some((literal, distance)) = &fixed {
                        self.compressed(bits, literal, distance)?;
                    }
                }
                Some(BlockType::Dynamic) => {
                    let (literal, distance) = dynamic(bits, work)?;
                    self.compressed(bits, &literal, &distance)?;
                }
                None => return Err(syntax("reserved DEFLATE block type", bits.offset).into()),
            }
            if final_block {
                return Ok(());
            }
        }
    }

    fn stored(&mut self, bits: &mut Reader<'_>) -> Read<()> {
        bits.align();
        let length = bits.read(16)?;
        if bits.read(16)? != (length ^ MAX_STORED) {
            return Err(syntax("DEFLATE stored length mismatch", bits.offset).into());
        }
        let remaining = bits.input.len().saturating_sub(bits.offset);
        let count = remaining.min(length);
        let end = bits
            .offset
            .checked_add(count)
            .ok_or_else(|| crate::bounded::decode_limit(bits.offset))?;
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
        bits: &mut Reader<'_>,
        literal: &Decoder,
        distance: &Decoder,
    ) -> Read<()> {
        // Without prediction, decoded bytes already are the DEFLATE history.
        // Accumulate their checksum once per block rather than once per symbol.
        if self.history.is_none() {
            let start = self.output.buffer().len();
            let result = self.compressed_inner::<true>(bits, literal, distance);
            self.adler.update(&self.output.buffer()[start..]);
            return result;
        }
        self.compressed_inner::<false>(bits, literal, distance)
    }

    fn compressed_inner<const RAW: bool>(
        &mut self,
        bits: &mut Reader<'_>,
        literal: &Decoder,
        distance: &Decoder,
    ) -> Read<()> {
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
                END_OF_BLOCK => return Ok(()),
                symbol @ FIRST_LENGTH..=LAST_LENGTH => {
                    let index = usize::from(symbol - FIRST_LENGTH);
                    let base = LENGTH_BASE
                        .get(index)
                        .copied()
                        .ok_or_else(|| syntax("invalid DEFLATE length", bits.offset))?;
                    let extra = LENGTH_EXTRA
                        .get(index)
                        .copied()
                        .ok_or_else(|| syntax("invalid DEFLATE length", bits.offset))?;
                    let length = base + bits.read(extra)?;
                    // RFC 1951 3.2.5 assigns code 284 lengths 227..257;
                    // length 258 has its own code (285).
                    if symbol == LAST_LENGTH - 1 && length == MAX_MATCH {
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
        let mut scratch = [0; MAX_MATCH];
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

/// The fixed code's decoders (RFC 1951 3.2.6).
fn fixed_tables(work: &mut Meter, offset: usize) -> Result<(Decoder, Decoder)> {
    Ok((
        Decoder::new(
            &fixed_literal_lengths(),
            Alphabet::LiteralLength,
            work,
            offset,
        )?,
        Decoder::new(
            &[FIXED_DISTANCE_BITS; FIXED_DISTANCE_SYMBOLS],
            Alphabet::Distance,
            work,
            offset,
        )?,
    ))
}

/// Reads a dynamic block's header (RFC 1951 3.2.7) and returns its
/// literal/length and distance decoders.
fn dynamic(bits: &mut Reader<'_>, work: &mut Meter) -> Read<(Decoder, Decoder)> {
    let literals = bits.read(5)? + usize::from(FIRST_LENGTH);
    if literals > LITERAL_SYMBOLS {
        return Err(syntax("invalid DEFLATE literal count", bits.offset).into());
    }
    let distances = bits.read(5)? + 1;
    let count = bits.read(4)? + MIN_CODE_LENGTH_CODES;
    let mut code_lengths = [0; CODE_LENGTH_SYMBOLS];
    for &symbol in CODE_LENGTH_ORDER.iter().take(count) {
        let value =
            u8::try_from(bits.read(3)?).map_err(|_| syntax("invalid code length", bits.offset))?;
        if let Some(slot) = code_lengths.get_mut(symbol) {
            *slot = value;
        }
    }
    let codes = Decoder::new(&code_lengths, Alphabet::CodeLength, work, bits.offset)?;
    let total = literals + distances;
    // HDIST is five bits, so a header declares at most 32 distance lengths.
    let mut lengths = [0; LITERAL_SYMBOLS + FIXED_DISTANCE_SYMBOLS];
    let mut cursor = 0;
    while cursor < total {
        let symbol = codes.read(bits)?;
        let (value, repeat) = match u8::try_from(symbol) {
            Ok(length @ 0..=15) => (length, 1),
            Ok(REPEAT_PREVIOUS) => (
                lengths
                    .get(cursor.checked_sub(1).ok_or_else(|| {
                        syntax("Huffman repeat without previous length", bits.offset)
                    })?)
                    .copied()
                    .ok_or_else(|| syntax("Huffman repeat without previous length", bits.offset))?,
                bits.read(2)? + 3,
            ),
            Ok(REPEAT_ZERO) => (0, bits.read(3)? + 3),
            Ok(REPEAT_ZERO_LONG) => (0, bits.read(7)? + 11),
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
    if lengths.get(usize::from(END_OF_BLOCK)).copied().unwrap_or(0) == 0 {
        return Err(syntax("DEFLATE end code missing from alphabet", bits.offset).into());
    }
    let literal = Decoder::new(
        lengths
            .get(..literals)
            .ok_or_else(|| syntax("invalid literal alphabet", bits.offset))?,
        Alphabet::LiteralLength,
        work,
        bits.offset,
    )?;
    let distance = Decoder::new(
        lengths
            .get(literals..total)
            .ok_or_else(|| syntax("invalid distance alphabet", bits.offset))?,
        Alphabet::Distance,
        work,
        bits.offset,
    )?;
    Ok((literal, distance))
}
