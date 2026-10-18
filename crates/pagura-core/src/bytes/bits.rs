//! MSB-first bit reading with the byte-stuffing rules of JPEG and JPEG 2000.

use super::Cursor;
use crate::{Error, Position, Result};
use std::marker::PhantomData;

/// How a bit stream escapes bytes that would look like markers.
///
/// Implemented by [`Plain`] (no stuffing: CCITT, JBIG2 MMR and Huffman
/// data), [`JpegFf00`] (T.81 entropy-coded segments) and [`J2kFf7`]
/// (T.800 packet headers). It decides how each byte is loaded and what
/// alignment consumes; [`BitCursor`] does the rest.
pub trait Stuffing {
    /// Reads the next byte from `bytes` and returns it with the number of
    /// data bits it carries, in its low bits. `previous` is the byte loaded
    /// before it, or zero at the start.
    ///
    /// # Errors
    ///
    /// A `Syntax` error for truncated data or a stuffing violation.
    fn load(bytes: &mut Cursor<'_>, previous: u8) -> Result<(u8, u32)>;

    /// Consumes whatever the format places after a byte-aligned end of bit
    /// data, given the last byte loaded. Nothing by default.
    ///
    /// # Errors
    ///
    /// A `Syntax` error for truncated data or a stuffing violation.
    fn align(bytes: &mut Cursor<'_>, previous: u8) -> Result<()> {
        let _ = (bytes, previous);
        Ok(())
    }
}

/// No byte stuffing: every byte carries eight data bits.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Plain;

impl Stuffing for Plain {
    fn load(bytes: &mut Cursor<'_>, _previous: u8) -> Result<(u8, u32)> {
        Ok((bytes.u8()?, 8))
    }
}

/// JPEG entropy-coded data (T.81 B.1.1.5 and F.1.2.3): a data byte `0xFF`
/// is followed by a stuffed `0x00`. `0xFF` followed by anything else is a
/// marker, which bit data cannot contain.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JpegFf00;

impl Stuffing for JpegFf00 {
    fn load(bytes: &mut Cursor<'_>, _previous: u8) -> Result<(u8, u32)> {
        let at = bytes.position();
        let byte = bytes.u8()?;
        if byte == 0xff && bytes.u8().ok() != Some(0) {
            return Err(Error::syntax("unexpected marker in JPEG entropy data").at(at));
        }
        Ok((byte, 8))
    }
}

/// JPEG 2000 packet headers (T.800 B.10.1): a byte after `0xFF` carries only
/// seven data bits, and its most significant bit must be zero. When the last
/// header byte is `0xFF`, that stuffed byte still follows it, and alignment
/// consumes it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct J2kFf7;

impl J2kFf7 {
    fn stuffed(bytes: &mut Cursor<'_>) -> Result<u8> {
        let at = bytes.position();
        let byte = bytes.u8()?;
        if byte & 0x80 != 0 {
            return Err(Error::syntax("invalid JPEG 2000 packet bit stuffing").at(at));
        }
        Ok(byte)
    }
}

impl Stuffing for J2kFf7 {
    fn load(bytes: &mut Cursor<'_>, previous: u8) -> Result<(u8, u32)> {
        if previous == 0xff {
            Ok((Self::stuffed(bytes)?, 7))
        } else {
            Ok((bytes.u8()?, 8))
        }
    }

    fn align(bytes: &mut Cursor<'_>, previous: u8) -> Result<()> {
        if previous == 0xff {
            Self::stuffed(bytes)?;
        }
        Ok(())
    }
}

/// The bits [`BitCursor::align`] discarded from the last partly read byte.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Padding {
    value: u8,
    width: u32,
}

impl Padding {
    /// The discarded bits, in the low `width` bits.
    pub const fn value(self) -> u8 {
        self.value
    }

    /// How many bits were discarded, 0 to 7.
    pub const fn width(self) -> u32 {
        self.width
    }

    /// Whether every discarded bit was one, as JPEG requires (T.81 F.1.2.3).
    /// Vacuously true when nothing was discarded.
    pub const fn is_ones(self) -> bool {
        u16::from_be_bytes([0, self.value]) + 1 == 1 << self.width
    }
}

/// A most-significant-bit-first reader over a [`Cursor`], with the
/// byte-stuffing rules `S`.
///
/// Errors are positioned at the byte that failed to load. The cursor is
/// `Clone`, so a decoder can look ahead by reading from a copy.
///
/// ```
/// use pagura_core::bytes::{BitCursor, Cursor, JpegFf00};
///
/// // 0xFF 0x00 is one data byte 0xFF in JPEG entropy-coded data.
/// let mut bits = BitCursor::<JpegFf00>::new(Cursor::new(&[0xff, 0x00, 0b1010_0000]));
/// assert_eq!(bits.bits(12)?, 0xffa);
/// assert_eq!(bits.align()?.width(), 4);
/// assert!(bits.byte_cursor()?.at_end());
/// # Ok::<(), pagura_core::Error>(())
/// ```
#[derive(Debug, Clone)]
pub struct BitCursor<'a, S: Stuffing = Plain> {
    bytes: Cursor<'a>,
    /// The byte bits are read from, and the last byte loaded.
    byte: u8,
    /// Unread bits of `byte`, in its low bits.
    left: u32,
    /// Offset of `byte` in the data, for positions.
    byte_offset: usize,
    stuffing: PhantomData<S>,
}

impl<'a, S: Stuffing> BitCursor<'a, S> {
    /// A bit reader starting at `bytes`' next unread byte.
    pub const fn new(bytes: Cursor<'a>) -> Self {
        let byte_offset = bytes.offset();
        Self {
            bytes,
            byte: 0,
            left: 0,
            byte_offset,
            stuffing: PhantomData,
        }
    }

    fn load(&mut self) -> Result<()> {
        self.byte_offset = self.bytes.offset();
        let (byte, bits) = S::load(&mut self.bytes, self.byte)?;
        self.byte = byte;
        self.left = bits;
        Ok(())
    }

    /// Reads one bit.
    ///
    /// # Errors
    ///
    /// A `Syntax` error at the end of the data or for a stuffing violation.
    pub fn bit(&mut self) -> Result<bool> {
        Ok(self.bits(1)? == 1)
    }

    /// Reads `count` bits, at most 32, most significant first.
    ///
    /// # Errors
    ///
    /// An `InvalidArgument` error if `count` exceeds 32, otherwise as
    /// [`BitCursor::bit`].
    pub fn bits(&mut self, count: u32) -> Result<u32> {
        if count > 32 {
            return Err(Error::invalid_argument("bit count exceeds 32"));
        }
        // Each step takes at most 8 bits and the total is at most 32, so the
        // shift never discards a bit that was read.
        let mut value = 0_u32;
        let mut needed = count;
        while needed > 0 {
            if self.left == 0 {
                self.load()?;
            }
            let take = needed.min(self.left);
            self.left -= take;
            let chunk = (u32::from(self.byte) >> self.left) & ((1 << take) - 1);
            value = (value << take) | chunk;
            needed -= take;
        }
        Ok(value)
    }

    /// Discards the unread bits of the current byte and returns them, then
    /// lets the stuffing rule consume what follows aligned bit data.
    ///
    /// # Errors
    ///
    /// As [`Stuffing::align`].
    pub fn align(&mut self) -> Result<Padding> {
        let padding = Padding {
            value: self.byte & u8::MAX.checked_shr(8 - self.left).unwrap_or(0),
            width: self.left,
        };
        self.left = 0;
        S::align(&mut self.bytes, self.byte)?;
        self.byte = 0;
        Ok(padding)
    }

    /// Whether no bits of a loaded byte remain unread.
    pub const fn is_aligned(&self) -> bool {
        self.left == 0
    }

    /// The position of the byte holding the next unread bit.
    pub const fn position(&self) -> Position {
        if self.left == 0 {
            self.bytes.position()
        } else {
            self.bytes.position_of(self.byte_offset)
        }
    }

    /// An upper bound on the unread bits: exact for [`Plain`], larger when
    /// stuffing removes bits that are still unread.
    pub const fn remaining_bits(&self) -> usize {
        self.bytes
            .remaining()
            .saturating_mul(8)
            .saturating_add(self.left as usize)
    }

    /// The byte cursor after the last byte loaded, to continue with
    /// byte-oriented data (markers, segment payloads) once aligned.
    ///
    /// # Errors
    ///
    /// An `InvalidArgument` error if bits of a loaded byte are still unread;
    /// call [`BitCursor::align`] first.
    pub fn byte_cursor(&mut self) -> Result<&mut Cursor<'a>> {
        if self.left != 0 {
            return Err(Error::invalid_argument("bit cursor is not byte-aligned"));
        }
        Ok(&mut self.bytes)
    }
}

impl<'a, S: Stuffing> From<Cursor<'a>> for BitCursor<'a, S> {
    fn from(bytes: Cursor<'a>) -> Self {
        Self::new(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::{BitCursor, J2kFf7, JpegFf00, Padding, Plain};
    use crate::bytes::Cursor;
    use crate::{ErrorKind, Position};

    #[test]
    fn plain_bits_cross_byte_boundaries_msb_first() {
        let data = [0b1011_0011, 0b0101_1100, 0xff, 0x12, 0x34, 0x56, 0x78];
        let mut bits = BitCursor::<Plain>::new(Cursor::new(&data));
        assert!(bits.bit().unwrap());
        assert_eq!(bits.bits(3).unwrap(), 0b011);
        assert_eq!(bits.position(), Position::Buffer(0));
        assert_eq!(bits.bits(6).unwrap(), 0b00_1101);
        assert_eq!(bits.remaining_bits(), 6 + 5 * 8);
        assert_eq!(bits.bits(0).unwrap(), 0);
        assert_eq!(bits.bits(6).unwrap(), 0b01_1100);
        assert!(bits.is_aligned());
        assert_eq!(bits.position(), Position::Buffer(2));
        assert_eq!(bits.bits(32).unwrap(), 0xff12_3456);
        assert_eq!(bits.bits(33).unwrap_err().kind, ErrorKind::InvalidArgument);
        assert_eq!(bits.bits(8).unwrap(), 0x78);
        let error = bits.bit().unwrap_err();
        assert_eq!(
            (error.kind, error.position),
            (ErrorKind::Syntax, Position::Buffer(7))
        );
    }

    #[test]
    fn alignment_returns_the_discarded_padding() {
        let mut bits = BitCursor::<Plain>::new(Cursor::new(&[0b1100_0111, 0xaa]));
        assert_eq!(bits.align().unwrap(), Padding::default());
        assert!(Padding::default().is_ones());
        assert_eq!(bits.bits(3).unwrap(), 0b110);
        assert!(bits.byte_cursor().is_err());
        let padding = bits.align().unwrap();
        assert_eq!((padding.value(), padding.width()), (0b0_0111, 5));
        assert!(!padding.is_ones());
        assert_eq!(bits.byte_cursor().unwrap().u8().unwrap(), 0xaa);
        let mut ones = BitCursor::<Plain>::new(Cursor::new(&[0b1011_1111]));
        ones.bits(2).unwrap();
        assert!(ones.align().unwrap().is_ones());
    }

    #[test]
    fn jpeg_stuffing_removes_zero_after_ff_and_rejects_markers() {
        let mut bits = BitCursor::<JpegFf00>::new(Cursor::new(&[0xff, 0x00, 0x80]));
        assert_eq!(bits.bits(9).unwrap(), 0x1ff);
        assert_eq!(bits.position(), Position::Buffer(2));
        for data in [[0x12, 0xff, 0xd9].as_slice(), &[0x12, 0xff]] {
            let mut bits = BitCursor::<JpegFf00>::new(Cursor::new(data));
            bits.bits(8).unwrap();
            let error = bits.bit().unwrap_err();
            assert_eq!(error.message, "unexpected marker in JPEG entropy data");
            assert_eq!(error.position, Position::Buffer(1));
        }
    }

    #[test]
    fn j2k_stuffing_takes_seven_bits_after_ff_and_aligns_past_it() {
        let data = [0xff, 0x7f, 0x80];
        let mut bits = BitCursor::<J2kFf7>::new(Cursor::new(&data));
        assert_eq!(bits.bits(15).unwrap(), 0x7fff);
        assert_eq!(bits.bits(8).unwrap(), 0x80);
        let error = BitCursor::<J2kFf7>::new(Cursor::new(&[0xff, 0x80]))
            .bits(9)
            .unwrap_err();
        assert_eq!(error.message, "invalid JPEG 2000 packet bit stuffing");
        assert_eq!(error.position, Position::Buffer(1));
        // A header that ends on 0xFF is followed by its stuffed byte.
        let mut bits = BitCursor::<J2kFf7>::new(Cursor::new(&[0xff, 0x00, 0x42]));
        bits.bits(8).unwrap();
        assert_eq!(bits.align().unwrap().width(), 0);
        assert_eq!(bits.byte_cursor().unwrap().u8().unwrap(), 0x42);
        let mut bits = BitCursor::<J2kFf7>::new(Cursor::new(&[0xff, 0x80]));
        bits.bits(8).unwrap();
        assert_eq!(bits.align().unwrap_err().kind, ErrorKind::Syntax);
    }

    #[test]
    fn clones_look_ahead_without_consuming() {
        let bits = BitCursor::<Plain>::from(Cursor::new(&[0b1010_0000]));
        let mut probe = bits.clone();
        assert_eq!(probe.bits(4).unwrap(), 0b1010);
        let mut bits = bits;
        assert_eq!(bits.bits(2).unwrap(), 0b10);
    }
}
