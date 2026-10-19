//! DEFLATE's least-significant-bit-first packing (RFC 1951 3.1.1), which
//! `pagura_core::bytes::BitCursor` (most significant bit first) does not
//! cover.
use crate::bounded::BoundedBytes;
use pagura_core::{Error, Result};

/// Why decoding stopped early.
#[derive(Debug)]
pub(super) enum Fault {
    /// The input ended inside the data, which is a reported truncation.
    End,
    /// The data is invalid.
    Invalid(Error),
}

impl From<Error> for Fault {
    fn from(error: Error) -> Self {
        Self::Invalid(error)
    }
}

pub(super) type Read<T> = std::result::Result<T, Fault>;

/// Reads bit fields, the first bit being the least significant.
pub(super) struct Reader<'a> {
    pub(super) input: &'a [u8],
    /// The byte holding the next unread bit.
    pub(super) offset: usize,
    /// Bits of that byte already read.
    bit: u8,
}

impl<'a> Reader<'a> {
    pub(super) const fn new(input: &'a [u8], offset: usize) -> Self {
        Self {
            input,
            offset,
            bit: 0,
        }
    }

    /// The next `width` bits (at most 16) and how many of them the input
    /// actually holds; missing bits read as zero.
    pub(super) fn peek(&self, width: u8) -> (usize, u8) {
        if let Some(bytes) = self
            .input
            .get(self.offset..)
            .and_then(|tail| tail.first_chunk::<4>())
        {
            let value = u32::from_le_bytes(*bytes) >> self.bit;
            return (
                usize::try_from(value & ((1 << width) - 1)).unwrap_or(0),
                width,
            );
        }
        let mut value = 0_usize;
        let mut available = 0;
        for index in 0..3 {
            let Some(&byte) = self
                .offset
                .checked_add(index)
                .and_then(|i| self.input.get(i))
            else {
                break;
            };
            value |= usize::from(byte) << (index * 8);
            available += 8;
        }
        (
            value >> self.bit & ((1 << width) - 1),
            (available - self.bit).min(width),
        )
    }

    pub(super) fn read(&mut self, width: u8) -> Read<usize> {
        let (value, available) = self.peek(width);
        if available < width {
            return Err(Fault::End);
        }
        self.advance(width);
        Ok(value)
    }

    pub(super) fn advance(&mut self, width: u8) {
        let total = self.bit + width;
        self.offset += usize::from(total / 8);
        self.bit = total % 8;
    }

    /// Skips to the next byte boundary.
    pub(super) fn align(&mut self) {
        if self.bit != 0 {
            self.offset += 1;
            self.bit = 0;
        }
    }
}

/// Writes bit fields, the first bit being the least significant.
pub(super) struct Writer {
    pub(super) bytes: BoundedBytes,
    pending: u32,
    /// Bits in `pending`, below eight between calls.
    pub(super) count: u8,
}

impl Writer {
    pub(super) const fn new(bytes: BoundedBytes) -> Self {
        Self {
            bytes,
            pending: 0,
            count: 0,
        }
    }

    /// Writes the low `width` bits of `value` (at most 24).
    pub(super) fn put(&mut self, value: u32, width: u8) -> Result<()> {
        self.pending |= value << self.count;
        self.count += width;
        while self.count >= 8 {
            self.bytes.push(self.pending.to_le_bytes()[0])?;
            self.pending >>= 8;
            self.count -= 8;
        }
        Ok(())
    }

    /// Pads with zero bits to the next byte boundary.
    pub(super) fn align(&mut self) -> Result<()> {
        if self.count != 0 {
            self.bytes.push(self.pending.to_le_bytes()[0])?;
            self.pending = 0;
            self.count = 0;
        }
        Ok(())
    }
}
