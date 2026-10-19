//! T.81 canonical Huffman codes and entropy byte stuffing.
mod fused;

use pagura_core::{Diagnostic, Diagnostics, Error, Position, Result};

pub(super) struct Huffman {
    short: [(u8, u8); 256],
    /// Twelve-bit prefixes with fused magnitudes for native planes; empty
    /// unless requested.
    wide: Vec<u32>,
    first: [u32; 16],
    count: [u8; 16],
    offset: [usize; 16],
    symbols: Vec<u8>,
}
impl Huffman {
    pub(super) fn symbol(&self, bits: &mut Bits<'_>) -> Result<u8> {
        if let Some(symbol) = bits.short(&self.short) {
            return Ok(symbol);
        }
        let mut code = 0;
        for length in 0..16 {
            code = code * 2 + bits.read(1)?;
            if let Some(relative) = code
                .checked_sub(self.first[length])
                .filter(|&v| v < u32::from(self.count[length]))
            {
                return Ok(self.symbols[self.offset[length] + relative as usize]);
            }
        }
        Err(bits.error("invalid JPEG Huffman code"))
    }
}

pub(super) fn tables(
    mut payload: &[u8],
    tables: &mut [Option<Huffman>; 8],
    wide: bool,
) -> Result<()> {
    while let Some((&info, rest)) = payload.split_first() {
        if info >> 4 > 1 || info & 15 > 3 {
            return Err(Error::syntax("invalid JPEG Huffman table selector"));
        }
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
        let mut short = [(0, 0); 256];
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
                if length <= 8 {
                    let start = (code << (8 - length)) as usize;
                    short[start..start + (1 << (8 - length))].fill((symbol, length));
                }
                index += 1;
                code += 1;
            }
            code *= 2;
        }
        let prefixes = if wide {
            fused::wide_table(&first, &count_by_length, &offsets, symbols, info >> 4 == 1).to_vec()
        } else {
            Vec::new()
        };
        tables[usize::from(info >> 4) * 4 + usize::from(info & 15)] = Some(Huffman {
            short,
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

pub(super) struct Bits<'a> {
    data: &'a [u8],
    pub(super) offset: usize,
    byte: u8,
    left: u8,
    /// The unstuffed segment used by native-plane decoding.
    stream: fused::Stream,
}
impl<'a> Bits<'a> {
    pub(super) fn new(data: &'a [u8], offset: usize) -> Self {
        Self {
            data,
            offset,
            byte: 0,
            left: 0,
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
    pub(super) fn error(&self, message: &'static str) -> Error {
        Error::syntax(message).at(Position::Buffer(fused::position(self)))
    }
    /// Inspect and consume a short code with one byte/stuffing check. No
    /// state changes until the table entry is known to fit before the marker.
    fn short(&mut self, table: &[(u8, u8); 256]) -> Option<u8> {
        let next = if self.left == 8 {
            self.byte
        } else {
            let next = *self.data.get(self.offset)?;
            if next == 255 && self.data.get(self.offset + 1) != Some(&0) {
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
        let (symbol, count) = table[usize::from(prefix)];
        if count == 0 {
            return None;
        }
        if count > self.left {
            self.byte = next;
            self.offset += 1 + usize::from(next == 255);
            self.left += 8;
        }
        self.left -= count;
        Some(symbol)
    }

    pub(super) fn read(&mut self, mut count: u8) -> Result<u32> {
        let mut value = 0;
        while count != 0 {
            if self.left == 0 {
                self.byte = *self
                    .data
                    .get(self.offset)
                    .ok_or(self.error("truncated JPEG entropy data"))?;
                if self.byte == 255 {
                    if self.data.get(self.offset + 1) != Some(&0) {
                        return Err(self.error("unexpected marker in JPEG entropy data"));
                    }
                    self.offset += 1;
                }
                self.offset += 1;
                self.left = 8;
            }
            let take = count.min(self.left);
            self.left -= take;
            value = (value << take) | (u32::from(self.byte >> self.left) & ((1 << take) - 1));
            count -= take;
        }
        Ok(value)
    }
    pub(super) fn align(
        &mut self,
        diagnostics: &mut Diagnostics,
        reported: &mut bool,
    ) -> Result<()> {
        fused::sync(self);
        if self.left > 0
            && self.byte & (255 >> (8 - self.left)) != (255 >> (8 - self.left))
            && !*reported
        {
            diagnostics.repair(
                Diagnostic::new(
                    super::tags::ENTROPY_PADDING,
                    "non-one JPEG entropy padding ignored (T.81 B.1.1.5, E.1.4)",
                )
                .at(Position::Buffer(self.offset.saturating_sub(1))),
            )?;
            *reported = true;
        }
        self.left = 0;
        Ok(())
    }
    pub(super) fn restart(
        &mut self,
        sequence: u8,
        diagnostics: &mut Diagnostics,
        reported: &mut bool,
    ) -> Result<()> {
        self.align(diagnostics, reported)?;
        if super::marker(self.data, &mut self.offset)? != 0xd0 + sequence {
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
    pub(super) fn signed(&mut self, count: u8) -> Result<i32> {
        let value =
            i32::try_from(self.read(count)?).map_err(|_| self.error("JPEG magnitude overflow"))?;
        Ok(if count > 0 && value < 1 << (count - 1) {
            value + 1 - (1 << count)
        } else {
            value
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Bits, tables};

    #[test]
    fn every_code_length_and_alignment_stops_before_the_marker() {
        // One canonical code of length L is L zero bits (T.81 C.2).
        for length in 1..=16u8 {
            let mut payload = vec![0; 17];
            payload[usize::from(length)] = 1;
            payload.push(122);
            let mut books = std::array::from_fn(|_| None);
            tables(&payload, &mut books, false).unwrap();
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
                assert_eq!(bits.offset, size);
                assert_eq!(
                    usize::from(bits.left),
                    size * 8 - leading - usize::from(length)
                );
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
            }
        }
        let mut bits = Bits::new(&[255, 217], 0);
        assert_eq!(bits.short(&[(0, 0); 256]), None);
        assert!(bits.read(1).is_err());
        assert_eq!(bits.offset, 0);
    }
}
