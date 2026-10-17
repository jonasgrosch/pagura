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
