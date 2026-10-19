//! Least-significant-bit-first DEFLATE input (RFC 1951 3.1.1).
//!
//! A 64-bit buffer is refilled with one unaligned little-endian load while
//! eight input bytes remain, and byte by byte near the end of the input.
//! Bits at and above `count` are zeros or the input bits that follow, so a
//! later refill may OR the same bytes in again, and a lookup past the end of
//! the input sees zero padding.
use super::{Fault, InflateResult};

/// After a word refill `count` is 56 plus its bits beyond whole bytes.
pub(super) const REFILLED: u32 = 0b11_1000;

pub(super) struct Bits<'a> {
    pub(super) input: &'a [u8],
    /// The first input byte not yet loaded into `buffer`.
    pub(super) next: usize,
    /// Unconsumed bits, the next one least significant.
    pub(super) buffer: u64,
    /// How many bits of `buffer` are loaded; always below 64.
    pub(super) count: u32,
}

impl<'a> Bits<'a> {
    pub(super) fn new(input: &'a [u8], start: usize) -> Self {
        Self {
            input,
            next: start,
            buffer: 0,
            count: 0,
        }
    }

    /// The byte holding the next unconsumed bit, as positioned errors report it.
    pub(super) fn offset(&self) -> usize {
        self.next - self.count.div_ceil(8) as usize
    }

    /// Loads input until at least 56 bits are buffered or the input ends.
    pub(super) fn refill(&mut self) {
        if let Some(word) = self
            .input
            .get(self.next..)
            .and_then(|tail| tail.first_chunk::<8>())
        {
            self.buffer |= u64::from_le_bytes(*word) << self.count;
            // Whole bytes only: `count | 56` is `count` plus 8 * that many.
            self.next += (63 - self.count) as usize / 8;
            self.count |= REFILLED;
            return;
        }
        while self.count <= 55
            && let Some(&byte) = self.input.get(self.next)
        {
            self.buffer |= u64::from(byte) << self.count;
            self.next += 1;
            self.count += 8;
        }
    }

    pub(super) fn consume(&mut self, width: u32) {
        self.buffer >>= width;
        self.count -= width;
    }

    /// Reads `width` (at most 16) bits, or reports the end of the input.
    pub(super) fn read(&mut self, width: u32) -> InflateResult<u32> {
        if self.count < width {
            self.refill();
            if self.count < width {
                return Err(Fault::End);
            }
        }
        let value = low_bits(self.buffer, width);
        self.consume(width);
        Ok(value)
    }

    /// Skips to the next byte boundary (RFC 1951 3.2.4).
    pub(super) fn align(&mut self) {
        self.consume(self.count % 8);
    }

    /// The input position once aligned; buffered whole bytes are unread.
    pub(super) fn aligned_offset(&self) -> usize {
        self.next - (self.count / 8) as usize
    }

    /// Continues at byte `offset`, discarding buffered bits.
    pub(super) fn restart(&mut self, offset: usize) {
        self.next = offset;
        self.buffer = 0;
        self.count = 0;
    }
}

/// The low `width` bits of `value`, for `width` below 32.
pub(super) fn low_bits(value: u64, width: u32) -> u32 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the mask keeps fewer than 32 bits"
    )]
    let low = (value & !u64::MAX.wrapping_shl(width)) as u32;
    low
}
