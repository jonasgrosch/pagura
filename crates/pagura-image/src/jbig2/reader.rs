//! Checked big-endian segment fields (T.88 clause 7).
use pagura_core::{Error, Position, Result};

pub(super) struct Reader<'a> {
    pub(super) data: &'a [u8],
    pub(super) offset: usize,
    base: usize,
}

impl<'a> Reader<'a> {
    pub(super) fn new(data: &'a [u8], base: usize) -> Self {
        Self {
            data,
            offset: 0,
            base,
        }
    }

    /// Segment body framing and unknown-length row counts, 7.2.7/7.4.6.4.
    pub(super) fn segment(&mut self, kind: u8, length: u32) -> Result<(Self, Option<u32>)> {
        let base = match self.position() {
            Position::Buffer(p) => p,
            _ => 0,
        };
        let mut actual_rows = None;
        let body = if length == u32::MAX {
            if kind != 38 {
                return Err(Error::syntax(
                    "unspecified length of JBIG2 non-generic segment",
                ));
            }
            let start = self.offset;
            let coding = *self
                .data
                .get(start + 17)
                .ok_or_else(|| Error::syntax("truncated JBIG2 unknown-length region"))?;
            let arithmetic = coding & 1 == 0;
            let header = 18
                + if arithmetic {
                    if coding & 6 == 0 { 8 } else { 2 }
                } else {
                    0
                };
            let marker = if arithmetic { [255, 172] } else { [0, 0] };
            let tail = self
                .data
                .get(start + header..)
                .ok_or_else(|| Error::syntax("truncated JBIG2 unknown-length header"))?;
            let end = tail
                .windows(2)
                .position(|pair| pair == marker)
                .ok_or_else(|| Error::syntax("unterminated JBIG2 unknown-length region"))?
                + header;
            let payload_length = end + if arithmetic { 2 } else { 0 };
            let body = self.take(payload_length)?;
            if !arithmetic {
                self.take(2)?;
            }
            actual_rows = Some(self.u32()?);
            body
        } else {
            self.take(
                usize::try_from(length).map_err(|_| Error::limit("JBIG2 segment size overflow"))?,
            )?
        };
        Ok((Reader::new(body, base), actual_rows))
    }

    pub(super) fn position(&self) -> Position {
        Position::Buffer(self.base + self.offset)
    }

    pub(super) fn take(&mut self, length: usize) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or_else(|| Error::limit("JBIG2 field size overflow").at(self.position()))?;
        let value = self
            .data
            .get(self.offset..end)
            .ok_or_else(|| Error::syntax("truncated JBIG2 field").at(self.position()))?;
        self.offset = end;
        Ok(value)
    }

    pub(super) fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub(super) fn signed_byte(&mut self) -> Result<i32> {
        Ok(i32::from(i8::from_be_bytes([self.byte()?])))
    }

    pub(super) fn u16(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    pub(super) fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub(super) fn finish(&self) -> Result<()> {
        if self.offset != self.data.len() {
            return Err(Error::syntax("trailing JBIG2 segment data").at(self.position()));
        }
        Ok(())
    }

    pub(super) fn rest(&mut self) -> &'a [u8] {
        let value = &self.data[self.offset..];
        self.offset = self.data.len();
        value
    }
}
