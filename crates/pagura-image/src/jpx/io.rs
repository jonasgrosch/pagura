//! Checked binary access and the shared allocation/work budget.

use pagura_core::{Error, Limits, Position, Result};

pub(super) fn syntax(message: &'static str, offset: usize) -> Error {
    Error::syntax(message).at(Position::Buffer(offset))
}

pub(super) struct Reader<'a> {
    pub data: &'a [u8],
    pub offset: usize,
    pub base: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8], base: usize) -> Self {
        Self {
            data,
            offset: 0,
            base,
        }
    }

    pub fn position(&self) -> usize {
        self.base + self.offset
    }

    pub fn remaining(&self) -> usize {
        self.data.len() - self.offset
    }

    pub fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(count)
            .ok_or_else(|| syntax("JPEG 2000 offset overflow", self.position()))?;
        let bytes = self
            .data
            .get(self.offset..end)
            .ok_or_else(|| syntax("truncated JPEG 2000 data", self.position()))?;
        self.offset = end;
        Ok(bytes)
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    pub fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn finish(&self) -> Result<()> {
        if self.remaining() == 0 {
            Ok(())
        } else {
            Err(syntax("extra JPEG 2000 field data", self.position()))
        }
    }
}

pub(super) struct Budget<'a> {
    pub limits: &'a Limits,
    bytes: usize,
    work: usize,
}

impl<'a> Budget<'a> {
    pub fn new(limits: &'a Limits) -> Self {
        Self {
            limits,
            bytes: 0,
            work: 0,
        }
    }

    pub fn work(&mut self, count: usize) -> Result<()> {
        self.work = self
            .work
            .checked_add(count)
            .filter(|&n| n <= self.limits.operations)
            .ok_or_else(|| Error::limit("JPEG 2000 work budget exceeded"))?;
        Ok(())
    }

    pub fn charge<T>(&mut self, count: usize) -> Result<()> {
        // Initializing or copying each allocation is work even before entropy decoding.
        self.work(count)?;
        self.bytes = count
            .checked_mul(size_of::<T>())
            .and_then(|n| self.bytes.checked_add(n))
            .filter(|&n| n <= self.limits.decoded_bytes)
            .ok_or_else(|| Error::limit("JPEG 2000 allocation budget exceeded"))?;
        Ok(())
    }

    pub fn filled<T: Clone>(&mut self, count: usize, value: T) -> Result<Vec<T>> {
        self.charge::<T>(count)?;
        let mut result = Vec::new();
        result
            .try_reserve_exact(count)
            .map_err(|_| Error::limit("JPEG 2000 allocation failed"))?;
        result.resize(count, value);
        Ok(result)
    }
}

pub(super) fn usize_from(value: u32) -> Result<usize> {
    usize::try_from(value).map_err(|_| Error::limit("JPEG 2000 size exceeds address space"))
}

pub(super) fn area(width: u32, height: u32) -> Result<usize> {
    usize_from(width)?
        .checked_mul(usize_from(height)?)
        .ok_or_else(|| Error::limit("JPEG 2000 sample count overflow"))
}
