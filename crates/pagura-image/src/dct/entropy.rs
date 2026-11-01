//! T.81 canonical Huffman codes and entropy byte stuffing.
//!
//! Entropy bits are read most significant first through a 64-bit window.
//! While eight bytes without 0xFF follow, one big-endian load refills it;
//! otherwise bytes are loaded singly, unstuffing FF 00 and stopping before a
//! marker or the end of the data (T.81 B.1.1.5, F.1.2.3). Errors and repairs
//! report the byte positions of a decoder that loads each byte only when it
//! needs one of its bits.
mod fused;
mod sequential;

use pagura_core::{Diagnostic, Diagnostics, Error, Position, Result};

/// Code bits resolved by one table lookup. Longer codes, and codes near a
/// marker, take the canonical decode of T.81 F.2.2.3.
const LOOKAHEAD: u32 = 9;

/// The longest Huffman code (T.81 C.2).
const LONGEST: u32 = 16;

/// A decoded Huffman code that is not yet consumed. Eight-byte alignment
/// lets a lookup index scale by a shift.
#[derive(Clone, Copy, Default)]
#[repr(align(8))]
pub(super) struct Code {
    /// A DC magnitude size, or an AC run and size (T.81 F.1.2.1, F.1.2.2).
    pub(super) symbol: u8,
    /// Code bits; zero in a lookup entry whose code is longer.
    length: u8,
    /// Code plus magnitude bits when the magnitude was decoded with the code
    /// (T.81 F.2.2.1); otherwise zero.
    fused: u8,
    /// Code plus magnitude bits in a lookup entry, whether or not the
    /// magnitude fits in the lookup; zero in an entry whose code is longer.
    total: u8,
    /// That magnitude's value, sign-extended (Table F.2).
    value: i16,
}

pub(super) struct Huffman {
    /// Indexed by the next [`LOOKAHEAD`] bits.
    lookup: Box<[Code; 1 << LOOKAHEAD]>,
    /// Twelve-bit prefixes with fused magnitudes for native planes; empty
    /// unless requested.
    wide: Vec<u32>,
    first: [u32; 16],
    count: [u8; 16],
    offset: [usize; 16],
    symbols: Vec<u8>,
}
impl Huffman {
    /// The next code. The caller consumes it with [`Bits::skip`] or
    /// [`Bits::magnitude`].
    #[expect(
        clippy::inline_always,
        reason = "every coefficient takes this path; it shares the block loop's registers"
    )]
    #[inline(always)]
    pub(super) fn code(&self, bits: &mut Bits<'_>) -> Result<Code> {
        bits.fill();
        let code = self.lookup[bits.peek(LOOKAHEAD)];
        if code.length != 0 && bits.count >= LOOKAHEAD {
            return Ok(code);
        }
        self.canonical(bits, code)
    }

    /// Decodes and consumes one symbol.
    pub(super) fn symbol(&self, bits: &mut Bits<'_>) -> Result<u8> {
        let code = self.code(bits)?;
        bits.skip(code);
        Ok(code.symbol)
    }

    /// Codes longer than the lookup, or whose lookup bits pass a marker.
    #[cold]
    fn canonical(&self, bits: &Bits<'_>, code: Code) -> Result<Code> {
        if code.length != 0 {
            // The lookup holds every code of up to LOOKAHEAD bits, but this
            // one, or its magnitude, extends past the loaded bits.
            return if u32::from(code.length) > bits.count {
                Err(bits.exhausted())
            } else if u32::from(code.fused) > bits.count {
                Ok(Code {
                    fused: 0,
                    value: 0,
                    ..code
                })
            } else {
                Ok(code)
            };
        }
        if let Some((symbol, length)) = self.long(bits.peek(LONGEST), bits.count) {
            return Ok(Code {
                symbol,
                length: u8::try_from(length).unwrap_or(0),
                ..Code::default()
            });
        }
        if bits.count < LONGEST {
            return Err(bits.exhausted());
        }
        // The bit-serial decode of T.81 F.2.2.3 has read all sixteen bits
        // when it rejects the code.
        let mut past = bits.clone();
        past.consume(LONGEST);
        Err(past.error("invalid JPEG Huffman code"))
    }

    /// The code of `symbol` and its length, if the table has one.
    pub(super) fn code_of(&self, symbol: u8) -> Option<(u64, u32)> {
        let index = self.symbols.iter().position(|&s| s == symbol)?;
        let (length, i) = (1..=LONGEST)
            .zip(0..)
            .find(|&(_, i)| index < self.offset[i] + usize::from(self.count[i]))?;
        let code = u64::from(self.first[i]) + u64::try_from(index - self.offset[i]).ok()?;
        Some((code, length))
    }

    /// The symbol and length of the code longer than the lookup that starts
    /// the sixteen bits `window`, among codes of at most `loaded` bits.
    fn long(&self, window: usize, loaded: u32) -> Option<(u8, u32)> {
        for length in LOOKAHEAD + 1..=LONGEST.min(loaded) {
            let i = length as usize - 1;
            if let Some(relative) = u32::try_from(window >> (LONGEST - length))
                .ok()
                .and_then(|code| code.checked_sub(self.first[i]))
                .filter(|&v| v < u32::from(self.count[i]))
            {
                return Some((self.symbols[self.offset[i] + relative as usize], length));
            }
        }
        None
    }
}

/// The value of a `size`-bit magnitude (T.81 F.2.2.1, Table F.2).
fn extend(raw: u32, size: u8) -> i32 {
    let value = i32::try_from(raw).unwrap_or(0);
    if size > 0 && value < 1 << (size - 1) {
        value + 1 - (1 << size)
    } else {
        value
    }
}

/// Parses Huffman table definitions. With `wide`, also builds native-plane
/// lookups, whose AC entries join coefficient pairs with `pairs`.
pub(super) fn tables(
    mut payload: &[u8],
    tables: &mut [Option<Huffman>; 8],
    (wide, pairs): (bool, bool),
) -> Result<()> {
    while let Some((&info, rest)) = payload.split_first() {
        if info >> 4 > 1 || info & 15 > 3 {
            return Err(Error::syntax("invalid JPEG Huffman table selector"));
        }
        let ac = info >> 4 == 1;
        let counts = rest
            .get(..16)
            .ok_or(Error::syntax("truncated JPEG Huffman counts"))?;
        let total: usize = counts.iter().map(|&v| usize::from(v)).sum();
        if total == 0 || total > 256 {
            return Err(Error::syntax("invalid JPEG Huffman table size"));
        }
        let symbols = rest
            .get(16..16 + total)
            .ok_or(Error::syntax("truncated JPEG Huffman symbols"))?;
        let mut lookup = Box::new([Code::default(); 1 << LOOKAHEAD]);
        let mut first = [0; 16];
        let mut count_by_length = [0; 16];
        let mut offsets = [0; 16];
        let (mut code, mut index) = (0u32, 0usize);
        let mut seen = [false; 256];
        for (length, &count) in (1u8..=16).zip(counts) {
            let i = usize::from(length - 1);
            first[i] = code;
            count_by_length[i] = count;
            offsets[i] = index;
            for _ in 0..count {
                if code >= (1 << length) - 1 {
                    return Err(Error::syntax(
                        "oversubscribed or all-ones JPEG Huffman code",
                    ));
                }
                let symbol = symbols[index];
                if seen[usize::from(symbol)] {
                    return Err(Error::syntax("duplicate JPEG Huffman symbol"));
                }
                seen[usize::from(symbol)] = true;
                if u32::from(length) <= LOOKAHEAD {
                    enter(&mut lookup, code, length, symbol, ac);
                }
                index += 1;
                code += 1;
            }
            code *= 2;
        }
        let prefixes = if wide {
            fused::wide_table(&first, &count_by_length, &offsets, symbols, (ac, pairs)).to_vec()
        } else {
            Vec::new()
        };
        tables[usize::from(info >> 4) * 4 + usize::from(info & 15)] = Some(Huffman {
            lookup,
            wide: prefixes,
            first,
            count: count_by_length,
            offset: offsets,
            symbols: symbols.to_vec(),
        });
        payload = &rest[16 + total..];
    }
    Ok(())
}

/// Enters one code into every lookup slot it prefixes. A DC symbol is a
/// magnitude size and an AC symbol a run and size (T.81 F.1.2.1, F.1.2.2);
/// when the magnitude also fits in the slot's bits, it is decoded here.
/// An AC size of zero is EOB or ZRL and has no magnitude.
fn enter(lookup: &mut [Code; 1 << LOOKAHEAD], code: u32, length: u8, symbol: u8, ac: bool) {
    let spare = LOOKAHEAD - u32::from(length);
    let start = (code << spare) as usize;
    let size = if ac { symbol & 15 } else { symbol };
    let fits = (!ac || size != 0) && u32::from(size) <= spare;
    for (suffix, slot) in (0u32..).zip(&mut lookup[start..start + (1 << spare)]) {
        *slot = Code {
            symbol,
            length,
            total: length.saturating_add(size),
            ..Code::default()
        };
        if fits {
            let raw = suffix >> (spare - u32::from(size));
            slot.fused = length + size;
            slot.value = i16::try_from(extend(raw, size)).unwrap_or(0);
        }
    }
}

#[derive(Clone)]
pub(super) struct Bits<'a> {
    data: &'a [u8],
    /// Where this entropy-coded segment starts (T.81 B.2.1).
    start: usize,
    /// The first byte not yet loaded into `window`.
    next: usize,
    /// Unconsumed bits, the next one most significant. Bits past `count`
    /// are zeros or the data bits that follow, so a later load may OR the
    /// same byte in again.
    window: u64,
    /// How many bits of `window` are loaded; always below 64.
    count: u32,
    /// The unstuffed segment used by native-plane decoding.
    stream: fused::Stream,
}
impl<'a> Bits<'a> {
    pub(super) fn new(data: &'a [u8], offset: usize) -> Self {
        Self {
            data,
            start: offset,
            next: offset,
            window: 0,
            count: 0,
            stream: fused::Stream::default(),
        }
    }

    /// Bounds the unstuffed segment copies of native-plane decoding by
    /// `storage` bytes and `work` operations.
    pub(super) fn limit_stream(&mut self, storage: usize, work: usize) {
        self.stream = fused::Stream::new(storage, work);
    }

    /// Operations charged by native-plane decoding in this scan.
    pub(super) fn stream_work(&self) -> usize {
        self.stream.work()
    }

    /// Just past the last byte with a consumed bit: loaded whole bytes are
    /// unread. Each loaded byte is one data byte or a stuffed FF 00 pair.
    pub(super) fn offset(&self) -> usize {
        let mut at = self.next;
        for _ in 0..self.count / 8 {
            let stuffed = at >= self.start + 2 && self.data[at - 2..at] == [255, 0];
            at -= if stuffed { 2 } else { 1 };
        }
        at
    }

    pub(super) fn error(&self, message: &'static str) -> Error {
        Error::syntax(message).at(Position::Buffer(fused::position(self)))
    }

    /// Bits are needed past the last loadable byte: every loaded bit has
    /// been consumed, so the error is at the byte that stopped loading.
    #[cold]
    fn exhausted(&self) -> Error {
        let message = if self.next < self.data.len() {
            "unexpected marker in JPEG entropy data"
        } else {
            "truncated JPEG entropy data"
        };
        Error::syntax(message).at(Position::Buffer(self.next))
    }

    fn cursor(&self) -> Cursor {
        Cursor {
            next: self.next,
            window: self.window,
            count: self.count,
        }
    }

    fn resume(&mut self, cursor: Cursor) {
        self.next = cursor.next;
        self.window = cursor.window;
        self.count = cursor.count;
    }

    /// As [`Cursor::fill`].
    #[expect(
        clippy::inline_always,
        reason = "every coefficient takes this path; it shares the block loop's registers"
    )]
    #[inline(always)]
    fn fill(&mut self) {
        let mut cursor = self.cursor();
        cursor.fill(self.data);
        self.resume(cursor);
    }

    /// The next `width` (1 to 16) bits, zero past the loaded ones.
    fn peek(&self, width: u32) -> usize {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "at most sixteen bits remain after the shift"
        )]
        let bits = (self.window >> (64 - width)) as usize;
        bits
    }

    fn consume(&mut self, width: u32) {
        self.window <<= width;
        self.count -= width;
    }

    /// Consumes a code without its magnitude.
    pub(super) fn skip(&mut self, code: Code) {
        self.consume(u32::from(code.length));
    }

    /// Consumes a code and its `size`-bit magnitude, returning the value.
    #[expect(
        clippy::inline_always,
        reason = "every coefficient takes this path; it shares the block loop's registers"
    )]
    #[inline(always)]
    pub(super) fn magnitude(&mut self, code: Code, size: u8) -> Result<i32> {
        if code.fused != 0 {
            self.consume(u32::from(code.fused));
            return Ok(i32::from(code.value));
        }
        self.skip(code);
        Ok(extend(self.read(size)?, size))
    }

    /// Reads `width` bits; entropy fields have at most 16.
    #[expect(
        clippy::inline_always,
        reason = "every coefficient takes this path; it shares the block loop's registers"
    )]
    #[inline(always)]
    pub(super) fn read(&mut self, width: u8) -> Result<u32> {
        let width = u32::from(width);
        if width == 0 {
            return Ok(0);
        }
        debug_assert!(width <= LONGEST);
        if self.count < width {
            self.fill();
            if self.count < width {
                return Err(self.exhausted());
            }
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "at most sixteen bits remain after the shift"
        )]
        let value = (self.window >> (64 - width)) as u32;
        self.consume(width);
        Ok(value)
    }

    pub(super) fn align(
        &mut self,
        diagnostics: &mut Diagnostics,
        reported: &mut bool,
    ) -> Result<()> {
        fused::sync(self);
        let left = self.count % 8;
        if left > 0 && self.window >> (64 - left) != (1 << left) - 1 && !*reported {
            diagnostics.repair(
                Diagnostic::new(
                    super::tags::ENTROPY_PADDING,
                    "non-one JPEG entropy padding ignored (T.81 B.1.1.5, E.1.4)",
                )
                .at(Position::Buffer(self.offset().saturating_sub(1))),
            )?;
            *reported = true;
        }
        self.next = self.offset();
        self.start = self.next;
        self.window = 0;
        self.count = 0;
        Ok(())
    }

    pub(super) fn restart(
        &mut self,
        sequence: u8,
        diagnostics: &mut Diagnostics,
        reported: &mut bool,
    ) -> Result<()> {
        self.align(diagnostics, reported)?;
        let marker = super::marker(self.data, &mut self.next)?;
        self.start = self.next;
        if marker != 0xd0 + sequence {
            return Err(self.error("wrong JPEG restart marker"));
        }
        fused::restart(self);
        Ok(())
    }

    /// Joins a zero DC difference with the following end of block.
    pub(super) fn native_shortcut(&mut self, dc: &Huffman, ac: &Huffman) -> Result<[u32; 4096]> {
        if dc.wide.len() != 4096 || ac.wide.len() != 4096 {
            return Err(self.error("missing JPEG Huffman prefixes"));
        }
        fused::shortcut(dc, ac, self)
    }

    /// Consumes a zero DC difference and end of block, if they come next.
    #[expect(
        clippy::inline_always,
        reason = "the background block test runs once per block"
    )]
    #[inline(always)]
    pub(super) fn native_zero(&mut self, shortcut: &[u32]) -> Result<bool> {
        fused::native_zero(self, shortcut)
    }

    /// Consumes the `length` bits `pattern`, if they come next.
    pub(super) fn native_pattern(&mut self, (pattern, length): (u64, u32)) -> Result<bool> {
        fused::native_pattern(self, pattern, length)
    }

    /// Decodes one baseline block into dequantized natural-order
    /// coefficients; returns whether any AC is nonzero and the L1 norm.
    pub(super) fn native_coefficients(
        &mut self,
        dc: &Huffman,
        ac: &Huffman,
        predictor: &mut i32,
        output: &mut [i32; 64],
        quant: &[u16; 64],
        shortcut: &[u32],
    ) -> Result<(bool, u64)> {
        fused::native(dc, ac, self, predictor, output, quant, shortcut)
    }

    /// Charges reference refinement work against this scan's stream budget.
    pub(super) fn charge_refinement(&mut self, work: usize) -> Result<()> {
        fused::charge(self, work)
    }
}

/// The load position and window of [`Bits`], held apart from it so that a
/// block loop keeps them in registers.
#[derive(Clone, Copy)]
struct Cursor {
    next: usize,
    window: u64,
    count: u32,
}

impl Cursor {
    /// Unless 32 bits are buffered, loads bytes until at least 56 are, or up
    /// to a marker or the end of the data.
    #[expect(
        clippy::inline_always,
        reason = "every coefficient takes this path; it shares the block loop's registers"
    )]
    #[inline(always)]
    fn fill(&mut self, data: &[u8]) {
        if self.count >= 32 {
            return;
        }
        if let Some(word) = data
            .get(self.next..)
            .and_then(|tail| tail.first_chunk::<8>())
            .filter(|word| !has_marker_byte(**word))
        {
            self.window |= u64::from_be_bytes(*word) >> self.count;
            // Whole bytes only.
            let bytes = (63 - self.count) / 8;
            self.next += bytes as usize;
            self.count += 8 * bytes;
            return;
        }
        *self = self.fill_bytes(data);
    }

    fn fill_bytes(mut self, data: &[u8]) -> Self {
        while self.count < 56
            && let Some(&byte) = data.get(self.next)
        {
            if byte == 255 {
                if data.get(self.next + 1) != Some(&0) {
                    break;
                }
                self.next += 1;
            }
            self.next += 1;
            self.window |= u64::from(byte) << (56 - self.count);
            self.count += 8;
        }
        self
    }
}

/// Whether eight bytes hold a 0xFF, which may start a marker: their
/// complement then has a zero byte, which the borrow test finds exactly.
fn has_marker_byte(word: [u8; 8]) -> bool {
    let inverted = !u64::from_ne_bytes(word);
    inverted.wrapping_sub(0x0101_0101_0101_0101) & !inverted & 0x8080_8080_8080_8080 != 0
}

#[cfg(test)]
mod tests {
    use super::{Bits, has_marker_byte, tables};

    #[test]
    fn every_code_length_and_alignment_stops_before_the_marker() {
        // One canonical code of length L is L zero bits (T.81 C.2).
        for length in 1..=16u8 {
            let mut payload = vec![0; 17];
            payload[usize::from(length)] = 1;
            payload.push(122);
            let mut books = std::array::from_fn(|_| None);
            tables(&payload, &mut books, (false, false)).unwrap();
            let book = books[0].as_ref().unwrap();
            for leading in 0..8usize {
                let size = (leading + usize::from(length)).div_ceil(8);
                let mut data = vec![255; size];
                for bit in leading..leading + usize::from(length) {
                    data[bit / 8] &= !(1 << (7 - bit % 8));
                }
                data.extend_from_slice(&[255, 217]);
                let mut bits = Bits::new(&data, 0);
                bits.read(u8::try_from(leading).unwrap()).unwrap();
                assert_eq!(book.symbol(&mut bits).unwrap(), 122);
                assert_eq!(bits.offset(), size);
                assert_eq!(
                    usize::try_from(bits.count).unwrap(),
                    size * 8 - leading - usize::from(length)
                );
                let error = bits.read(8).unwrap_err();
                assert_eq!(error.message, "unexpected marker in JPEG entropy data");
                assert_eq!(error.position, pagura_core::Position::Buffer(size));
            }
        }
    }

    #[test]
    fn byte_reads_match_the_logical_bits_and_preserve_stuffing() {
        let source = [0xa5, 255, 0, 0x51, 0x09];
        let expected = 0xa5ff_5109u32;
        for first in 0..=16 {
            for second in 0..=16 {
                let mut bits = Bits::new(&source, 0);
                let a = bits.read(first).unwrap();
                let b = bits.read(second).unwrap();
                let consumed = first + second;
                let actual = (a << second) | b;
                assert_eq!(
                    actual,
                    if consumed == 0 {
                        0
                    } else {
                        expected >> (32 - consumed)
                    }
                );
                // Raw bytes holding a consumed bit; FF 00 is one data byte.
                let bytes = usize::from(consumed).div_ceil(8);
                assert_eq!(bits.offset(), bytes + usize::from(bytes >= 2));
            }
        }
        let mut bits = Bits::new(&[255, 217], 0);
        let error = bits.read(1).unwrap_err();
        assert_eq!(error.message, "unexpected marker in JPEG entropy data");
        assert_eq!(bits.offset(), 0);
        let error = Bits::new(&[1], 0).read(9).unwrap_err();
        assert_eq!(error.message, "truncated JPEG entropy data");
        assert_eq!(error.position, pagura_core::Position::Buffer(1));
    }

    #[test]
    fn word_loads_stop_at_every_marker_byte_position() {
        for at in 0..8 {
            let mut word = [0x5a; 8];
            word[at] = 255;
            assert!(has_marker_byte(word));
            word[at] = 254;
            assert!(!has_marker_byte(word));
        }
        assert!(has_marker_byte([255; 8]));
        assert!(!has_marker_byte([0; 8]));
        // A stuffed byte in any lane is still read as one data byte.
        for at in 0..12 {
            let mut data = vec![0x33; 16];
            data[at] = 255;
            data.insert(at + 1, 0);
            data.extend_from_slice(&[255, 217]);
            let mut bits = Bits::new(&data, 0);
            for (i, &expected) in [0x33u8; 16]
                .iter()
                .enumerate()
                .map(|(i, v)| (i, if i == at { &255 } else { v }))
            {
                assert_eq!(bits.read(8).unwrap(), u32::from(expected), "{at} {i}");
            }
            assert_eq!(bits.offset(), 17);
            assert!(bits.read(1).is_err());
        }
    }

    /// The logical bits of `data` after FF 00 unstuffing, most significant first.
    fn logical(data: &[u8]) -> impl Iterator<Item = u32> + '_ {
        data.iter()
            .enumerate()
            .filter(|&(i, _)| !(i > 0 && data[i - 1] == 255))
            .flat_map(|(_, &byte)| (0..8).rev().map(move |bit| u32::from(byte >> bit & 1)))
    }

    #[test]
    fn lookup_codes_and_magnitudes_match_a_bit_serial_decode() {
        // One canonical code of every length 1..=16 (T.81 C.2).
        let dc: Vec<u8> = (0..16).collect();
        let ac = [
            0x01, 0x00, 0x02, 0x11, 0x03, 0xf0, 0x21, 0x04, 0x05, 0x31, 0x06, 0x41, 0x07, 0x08,
            0x0a, 0x09,
        ];
        for (class, symbols) in [(0u8, &dc[..]), (0x10, &ac[..])] {
            let mut payload = vec![class];
            payload.extend_from_slice(&[1; 16]);
            payload.extend_from_slice(symbols);
            let mut books = std::array::from_fn(|_| None);
            tables(&payload, &mut books, (false, false)).unwrap();
            let book = books[usize::from(class >> 4) * 4].as_ref().unwrap();
            for window in 0..=u16::MAX {
                let mut data = Vec::new();
                for byte in [window.to_be_bytes(), window.rotate_left(5).to_be_bytes()]
                    .concat()
                    .into_iter()
                    .chain([0x5a; 3])
                {
                    data.push(byte);
                    if byte == 255 {
                        data.push(0);
                    }
                }
                data.extend_from_slice(&[255, 217]);
                // With one code per length, each length's code is the
                // previous one plus one, doubled (T.81 C.2).
                let mut logical = logical(&data);
                let (mut code, mut first) = (0u32, 0u32);
                let mut found = None;
                for length in 1..=16u32 {
                    code = code * 2 + logical.next().unwrap();
                    if code == first {
                        found = Some((usize::try_from(length).unwrap() - 1, length));
                        break;
                    }
                    first = (first + 1) * 2;
                }
                let mut bits = Bits::new(&data, 0);
                let Some((index, length)) = found else {
                    let error = book.code(&mut bits).err().unwrap();
                    assert_eq!(error.message, "invalid JPEG Huffman code");
                    continue;
                };
                let symbol = symbols[index];
                let size = if class == 0 { symbol } else { symbol & 15 };
                let raw = (0..size).fold(0, |v, _| v * 2 + logical.next().unwrap());
                let expected = super::extend(raw, size);
                let next = (0..8).fold(0, |v, _| v * 2 + logical.next().unwrap());
                let decoded = book.code(&mut bits).unwrap();
                assert_eq!(decoded.symbol, symbol, "{window:#06x}");
                assert_eq!(u32::from(decoded.length), length);
                assert_eq!(bits.magnitude(decoded, size).unwrap(), expected);
                assert_eq!(bits.read(8).unwrap(), next, "{window:#06x}");
            }
        }
    }
}
