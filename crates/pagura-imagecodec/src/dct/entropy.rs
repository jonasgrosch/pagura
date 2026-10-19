//! T.81 canonical Huffman tables (Annex C) and the bits of Huffman-coded
//! entropy data (F.2.2), with T.81 B.1.1.5 byte stuffing.
use super::marker;
use pagura_core::bytes::Cursor;
use pagura_core::{Diagnostic, Diagnostics, Error, Position, Result};

/// The longest Huffman code, in bits (T.81 C.2).
const LONGEST: usize = 16;
/// Codes up to this length are decoded by one table lookup.
const SHORT: u8 = 8;

pub(super) struct Huffman {
    /// The symbol and length of each code of at most [`SHORT`] bits, at every
    /// index those bits prefix; length zero where a longer code starts.
    short: [(u8, u8); 1 << SHORT],
    first: [u32; LONGEST],
    count: [u8; LONGEST],
    offset: [usize; LONGEST],
    symbols: Vec<u8>,
}

impl Huffman {
    /// Decodes one symbol (T.81 F.2.2.3): a short code by lookup, a longer
    /// one, or one that ends just before a marker, a bit at a time.
    pub(super) fn symbol(&self, bits: &mut Bits<'_>) -> Result<u8> {
        if let Some(symbol) = bits.short(&self.short) {
            return Ok(symbol);
        }
        let at = bits.position();
        let mut code = 0;
        for length in 0..LONGEST {
            code = code * 2 + u32::from(bits.bit()?);
            if let Some(relative) = code
                .checked_sub(self.first[length])
                .filter(|&v| v < u32::from(self.count[length]))
            {
                return self
                    .symbols
                    .get(self.offset[length] + pagura_core::convert::u32_to_usize(relative))
                    .copied()
                    .ok_or(Error::syntax("invalid JPEG Huffman code").at(at));
            }
        }
        Err(Error::syntax("invalid JPEG Huffman code").at(at))
    }
}

/// The Huffman tables of a decode: four for DC and four for AC
/// coefficients (T.81 B.2.4.2).
#[derive(Default)]
pub(super) struct Tables {
    pub(super) dc: [Option<Huffman>; 4],
    pub(super) ac: [Option<Huffman>; 4],
}

/// Reads a DHT segment payload into `tables`.
pub(super) fn tables(mut bytes: Cursor<'_>, tables: &mut Tables) -> Result<()> {
    while !bytes.at_end() {
        let at = bytes.position();
        let info = bytes.u8()?;
        let class = match info >> 4 {
            0 => &mut tables.dc,
            1 => &mut tables.ac,
            _ => return Err(Error::syntax("invalid JPEG Huffman table selector").at(at)),
        };
        let table = class
            .get_mut(usize::from(info & 15))
            .ok_or(Error::syntax("invalid JPEG Huffman table selector").at(at))?;
        let counts: [u8; LONGEST] = bytes.take_array()?;
        let total: usize = counts.iter().map(|&v| usize::from(v)).sum();
        if total == 0 || total > 256 {
            return Err(Error::syntax("invalid JPEG Huffman table size").at(at));
        }
        let symbols = bytes.take(total)?;
        let mut short = [(0, 0); 1 << SHORT];
        let mut first = [0; LONGEST];
        let mut count_by_length = [0; LONGEST];
        let mut offsets = [0; LONGEST];
        let (mut code, mut index) = (0u32, 0usize);
        let mut seen = [false; 256];
        for ((length, &count), i) in (1u8..=16).zip(&counts).zip(0..) {
            first[i] = code;
            count_by_length[i] = count;
            offsets[i] = index;
            for &symbol in symbols
                .get(index..index + usize::from(count))
                .unwrap_or(&[])
            {
                if code >= (1 << length) - 1 {
                    return Err(
                        Error::syntax("oversubscribed or all-ones JPEG Huffman code").at(at),
                    );
                }
                if std::mem::replace(&mut seen[usize::from(symbol)], true) {
                    return Err(Error::syntax("duplicate JPEG Huffman symbol").at(at));
                }
                if length <= SHORT {
                    let start = pagura_core::convert::u32_to_usize(code << (SHORT - length));
                    if let Some(run) = short.get_mut(start..start + (1 << (SHORT - length))) {
                        run.fill((symbol, length));
                    }
                }
                code += 1;
            }
            index += usize::from(count);
            code *= 2;
        }
        *table = Some(Huffman {
            short,
            first,
            count: count_by_length,
            offset: offsets,
            symbols: symbols.to_vec(),
        });
    }
    Ok(())
}

/// The bits of Huffman-coded entropy data, loaded a byte at a time with the
/// byte stuffing of T.81 B.1.1.5: a data byte `0xFF` is followed by a
/// stuffed `0x00`, and `0xFF` followed by anything else is a marker.
pub(super) struct Bits<'a> {
    /// The entropy-coded data and the byte-oriented data after it.
    bytes: Cursor<'a>,
    data: &'a [u8],
    /// The offset of the next byte to load.
    offset: usize,
    /// The offset of `byte`, for positions.
    byte_offset: usize,
    byte: u8,
    /// Unread bits of `byte`, in its low bits.
    left: u8,
    /// Where offset `first` of `data` is in the decoder's input.
    start: Position,
    first: usize,
}

impl<'a> Bits<'a> {
    pub(super) fn new(bytes: Cursor<'a>) -> Self {
        let offset = bytes.offset();
        Self {
            data: bytes.data(),
            offset,
            byte_offset: offset,
            byte: 0,
            left: 0,
            start: bytes.position(),
            first: offset,
            bytes,
        }
    }

    /// The position of the byte holding the next unread bit.
    pub(super) const fn position(&self) -> Position {
        let offset = if self.left == 0 {
            self.offset
        } else {
            self.byte_offset
        };
        Position::Buffer(offset.saturating_sub(self.first)).within(self.start)
    }

    /// A syntax error at the byte holding the next unread bit.
    pub(super) fn error(&self, message: &'static str) -> Error {
        Error::syntax(message).at(self.position())
    }

    pub(super) fn bit(&mut self) -> Result<bool> {
        Ok(self.read(1)? == 1)
    }

    /// Decodes a code of at most [`SHORT`] bits by looking up the next
    /// [`SHORT`] bits in `table`, with one byte and stuffing check. Consumes
    /// nothing when those bits would reach a marker or the end of the data,
    /// or when a longer code starts.
    fn short(&mut self, table: &[(u8, u8); 1 << SHORT]) -> Option<u8> {
        let next = if self.left == 8 {
            self.byte
        } else {
            let next = *self.data.get(self.offset)?;
            if next == marker::PREFIX && self.data.get(self.offset + 1) != Some(&marker::STUFFED) {
                return None;
            }
            next
        };
        let prefix = if self.left == 8 {
            self.byte
        } else if self.left == 0 {
            next
        } else {
            (self.byte << (8 - self.left)) | (next >> self.left)
        };
        let (symbol, length) = table[usize::from(prefix)];
        if length == 0 {
            return None;
        }
        if length > self.left {
            self.byte = next;
            self.byte_offset = self.offset;
            self.offset += 1 + usize::from(next == marker::PREFIX);
            self.left += 8;
        }
        self.left -= length;
        Some(symbol)
    }

    /// Reads `count` bits, at most 16 (T.81 F.2.2.4).
    pub(super) fn read(&mut self, mut count: u8) -> Result<u32> {
        let mut value = 0;
        while count != 0 {
            if self.left == 0 {
                self.load()?;
            }
            let take = count.min(self.left);
            self.left -= take;
            value = (value << take) | (u32::from(self.byte >> self.left) & ((1 << take) - 1));
            count -= take;
        }
        Ok(value)
    }

    fn load(&mut self) -> Result<()> {
        let byte = *self
            .data
            .get(self.offset)
            .ok_or_else(|| self.error("truncated JPEG entropy data"))?;
        if byte == marker::PREFIX {
            if self.data.get(self.offset + 1) != Some(&marker::STUFFED) {
                return Err(self.error("unexpected marker in JPEG entropy data"));
            }
            self.offset += 1;
        }
        self.byte = byte;
        self.byte_offset = self.offset - usize::from(byte == marker::PREFIX);
        self.offset += 1;
        self.left = 8;
        Ok(())
    }

    /// Reads a `count`-bit magnitude category value (T.81 F.2.2.1, EXTEND).
    pub(super) fn signed(&mut self, count: u8) -> Result<i32> {
        let value =
            i32::try_from(self.read(count)?).map_err(|_| self.error("JPEG magnitude overflow"))?;
        Ok(if count > 0 && value < 1 << (count - 1) {
            value + 1 - (1 << count)
        } else {
            value
        })
    }

    /// Skips the padding before a marker, which T.81 B.1.1.5 fills with
    /// one bits; other padding is a reported repair, at most once per image.
    pub(super) fn align(
        &mut self,
        diagnostics: &mut Diagnostics,
        reported: &mut bool,
    ) -> Result<()> {
        // The unread low bits of the current byte, none when aligned.
        let ones = u8::MAX.checked_shr(u32::from(8 - self.left)).unwrap_or(0);
        if self.byte & ones != ones && !*reported {
            diagnostics.repair(
                Diagnostic::new(
                    crate::repairs::JPEG_ENTROPY_PADDING,
                    "non-one JPEG entropy padding ignored (T.81 B.1.1.5, E.1.4)",
                )
                .at(self.position()),
            )?;
            *reported = true;
        }
        self.left = 0;
        Ok(())
    }

    /// Reads restart marker `RST<sequence>` after the padding (T.81 E.1.4).
    pub(super) fn restart(
        &mut self,
        sequence: u8,
        diagnostics: &mut Diagnostics,
        reported: &mut bool,
    ) -> Result<()> {
        self.align(diagnostics, reported)?;
        let mut bytes = self.bytes.clone();
        bytes.seek(self.offset)?;
        let at = bytes.position();
        if super::read_marker(&mut bytes)? != marker::RST0 + sequence {
            return Err(Error::syntax("wrong JPEG restart marker").at(at));
        }
        self.offset = bytes.offset();
        Ok(())
    }

    /// The offset of the next byte, once aligned.
    pub(super) const fn offset(&self) -> usize {
        self.offset
    }
}

#[cfg(test)]
mod tests {
    use super::{Bits, Tables, tables};
    use pagura_core::Position;
    use pagura_core::bytes::Cursor;

    #[test]
    fn every_code_length_and_alignment_stops_before_the_marker() {
        // One canonical code of length L is L zero bits (T.81 C.2).
        for length in 1..=16u8 {
            let mut payload = vec![0; 17];
            payload[usize::from(length)] = 1;
            payload.push(122);
            let mut books = Tables::default();
            tables(Cursor::new(&payload), &mut books).unwrap();
            let book = books.dc[0].as_ref().unwrap();
            for leading in 0..8u8 {
                let used = usize::from(leading + length);
                let size = used.div_ceil(8);
                let mut data = vec![255; size];
                for bit in usize::from(leading)..used {
                    data[bit / 8] &= !(1 << (7 - bit % 8));
                }
                data.extend_from_slice(&[255, 217]);
                let mut bits = Bits::new(Cursor::new(&data));
                bits.read(leading).unwrap();
                assert_eq!(book.symbol(&mut bits).unwrap(), 122);
                assert_eq!(bits.offset, size);
                assert_eq!(usize::from(bits.left), size * 8 - used);
            }
        }
    }

    #[test]
    fn byte_reads_match_the_logical_bits_and_preserve_stuffing() {
        let source = [0xa5, 255, 0, 0x51, 0x09];
        let expected = 0xa5ff_5109u32;
        for first in 0..=16 {
            for second in 0..=16 {
                let mut bits = Bits::new(Cursor::new(&source));
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
            }
        }
        let mut bits = Bits::new(Cursor::new(&[255, 217]));
        assert_eq!(bits.short(&[(7, 1); 256]), None);
        assert_eq!(bits.position(), Position::Buffer(0));
        assert!(bits.read(1).is_err());
        assert_eq!(bits.offset, 0);
        // A stuffed 0xFF is data: 0xFF 0x00 0x80 holds 1111 1111 1000 0000.
        let mut bits = Bits::new(Cursor::new(&[255, 0, 128]));
        assert_eq!(bits.short(&[(7, 3); 256]), Some(7));
        assert_eq!(bits.read(7).unwrap(), 0b111_1110);
    }
}
