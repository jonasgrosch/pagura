//! T.81 canonical Huffman codes and entropy byte stuffing.
use pagura_core::{Diagnostic, Diagnostics, Error, Position, Result};

pub(super) struct Huffman {
    first: [u32; 16],
    count: [u8; 16],
    offset: [usize; 16],
    symbols: Vec<u8>,
}
impl Huffman {
    pub(super) fn symbol(&self, bits: &mut Bits<'_>) -> Result<u8> {
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

pub(super) fn tables(mut payload: &[u8], tables: &mut [Option<Huffman>; 8]) -> Result<()> {
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
                index += 1;
                code += 1;
            }
            code *= 2;
        }
        tables[usize::from(info >> 4) * 4 + usize::from(info & 15)] = Some(Huffman {
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
}
impl<'a> Bits<'a> {
    pub(super) fn new(data: &'a [u8], offset: usize) -> Self {
        Self {
            data,
            offset,
            byte: 0,
            left: 0,
        }
    }
    pub(super) fn error(&self, message: &'static str) -> Error {
        Error::syntax(message).at(Position::Buffer(self.offset))
    }
    pub(super) fn read(&mut self, count: u8) -> Result<u32> {
        let mut value = 0;
        for _ in 0..count {
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
            self.left -= 1;
            value = (value << 1) | u32::from((self.byte >> self.left) & 1);
        }
        Ok(value)
    }
    pub(super) fn align(
        &mut self,
        diagnostics: &mut Diagnostics,
        reported: &mut bool,
    ) -> Result<()> {
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
        Ok(())
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
