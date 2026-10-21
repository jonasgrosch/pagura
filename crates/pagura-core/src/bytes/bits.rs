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
    /// Whether every loaded byte carries eight data bits. Then byte
    /// boundaries follow from the bit count alone; otherwise [`BitCursor`]
    /// finds them by replaying the loads since its last alignment.
    const UNIFORM: bool = true;

    /// How many input bytes a loaded byte occupied, its stuffing included.
    /// Only consulted when [`Stuffing::UNIFORM`] holds.
    fn raw_len(byte: u8) -> usize {
        let _ = byte;
        1
    }

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
    fn raw_len(byte: u8) -> usize {
        if byte == 0xff { 2 } else { 1 }
    }

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
    const UNIFORM: bool = false;

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
/// Bits are buffered a 64-bit word at a time, so an entropy decoder can
/// [`peek`](BitCursor::peek) at a code, look it up and
/// [`consume`](BitCursor::consume) its length without cloning the cursor,
/// and [`bits`](BitCursor::bits) touches the input only when the buffer runs
/// low. Loading stops early at the first truncation or stuffing violation;
/// that error is returned when a read needs bits beyond it, positioned at
/// the byte that failed to load, so a decoder may read up to a JPEG marker
/// and then handle the marker through [`byte_cursor`](BitCursor::byte_cursor).
///
/// ```
/// use pagura_core::bytes::{BitCursor, Cursor, JpegFf00};
///
/// // 0xFF 0x00 is one data byte 0xFF in JPEG entropy-coded data.
/// let mut bits = BitCursor::<JpegFf00>::new(Cursor::new(&[0xff, 0x00, 0b1010_0000]));
/// assert_eq!(bits.peek(4), 0xf);
/// assert_eq!(bits.bits(12)?, 0xffa);
/// assert_eq!(bits.align()?.width(), 4);
/// assert!(bits.byte_cursor()?.at_end());
/// # Ok::<(), pagura_core::Error>(())
/// ```
#[derive(Debug, Clone)]
pub struct BitCursor<'a, S: Stuffing = Plain> {
    bytes: Cursor<'a>,
    /// Unconsumed bits in the low `count` bits, oldest first.
    buffer: u64,
    count: u32,
    /// The last byte loaded, which decides the stuffing of the next one.
    previous: u8,
    /// Why loading stopped before filling the buffer.
    stopped: Option<Error>,
    /// Input offset where `loaded` started counting, at a byte boundary.
    start: usize,
    /// `previous` at `start`.
    start_previous: u8,
    /// Data bits loaded since `start`.
    loaded: u64,
    stuffing: PhantomData<S>,
}

/// The bits of `word` below `width`, `width` at most 32.
#[expect(clippy::cast_possible_truncation, reason = "masked to at most 32 bits")]
const fn low(word: u64, width: u32) -> u32 {
    (word & ((1 << width) - 1)) as u32
}

/// Where the next unconsumed bit lies, found by replaying loads.
struct Location {
    /// Input offset of the byte holding the next bit.
    offset: usize,
    /// Bits of that byte already consumed.
    consumed: u32,
    /// The last byte loaded before it.
    previous: u8,
}

impl<'a, S: Stuffing> BitCursor<'a, S> {
    /// A bit reader starting at `bytes`' next unread byte.
    pub const fn new(bytes: Cursor<'a>) -> Self {
        let start = bytes.offset();
        Self {
            bytes,
            buffer: 0,
            count: 0,
            previous: 0,
            stopped: None,
            start,
            start_previous: 0,
            loaded: 0,
            stuffing: PhantomData,
        }
    }

    /// Loads bytes until the buffer holds more than 56 bits or loading stops.
    fn refill(&mut self) {
        if self.count == 0 {
            self.start = self.bytes.offset;
            self.start_previous = self.previous;
            self.loaded = 0;
        }
        while self.count <= 56 && self.stopped.is_none() {
            let before = self.bytes.offset;
            match S::load(&mut self.bytes, self.previous) {
                Ok((byte, width)) => {
                    self.buffer = (self.buffer << width) | u64::from(byte);
                    self.count += width;
                    self.loaded += u64::from(width);
                    self.previous = byte;
                }
                Err(error) => {
                    self.bytes.offset = before;
                    self.stopped = Some(error);
                }
            }
        }
    }

    /// The error for a read beyond the loadable bits.
    fn exhausted(&self) -> Error {
        self.stopped
            .clone()
            .unwrap_or(Error::invalid_argument("bit count exceeds 32"))
    }

    /// Data bits consumed since `start`.
    fn consumed(&self) -> u64 {
        self.loaded - u64::from(self.count)
    }

    /// Returns the next `count` bits, at most 32, without consuming them.
    /// Bits beyond the end of the loadable data read as zero, so a decoder
    /// can always look up a full-width code; [`BitCursor::consume`] then
    /// rejects consuming bits that do not exist.
    #[inline]
    pub fn peek(&mut self, count: u32) -> u32 {
        let count = count.min(32);
        if count > self.count {
            self.refill();
        }
        if count <= self.count {
            low(self.buffer >> (self.count - count), count)
        } else {
            low(self.buffer << (count - self.count), count)
        }
    }

    /// Consumes `count` bits, at most 32, typically after
    /// [`BitCursor::peek`].
    ///
    /// # Errors
    ///
    /// An `InvalidArgument` error if `count` exceeds 32, otherwise the
    /// truncation or stuffing error that stopped loading before `count`
    /// bits were available.
    #[inline]
    pub fn consume(&mut self, count: u32) -> Result<()> {
        if count > 32 {
            return Err(Error::invalid_argument("bit count exceeds 32"));
        }
        if count > self.count {
            self.refill();
            if count > self.count {
                return Err(self.exhausted());
            }
        }
        self.count -= count;
        Ok(())
    }

    /// Reads one bit.
    ///
    /// # Errors
    ///
    /// As [`BitCursor::bits`].
    #[inline]
    pub fn bit(&mut self) -> Result<bool> {
        Ok(self.bits(1)? == 1)
    }

    /// Reads `count` bits, at most 32, most significant first.
    ///
    /// # Errors
    ///
    /// As [`BitCursor::consume`].
    #[inline]
    pub fn bits(&mut self, count: u32) -> Result<u32> {
        if count > 32 {
            return Err(Error::invalid_argument("bit count exceeds 32"));
        }
        if count > self.count {
            self.refill();
            if count > self.count {
                return Err(self.exhausted());
            }
        }
        self.count -= count;
        Ok(low(self.buffer >> self.count, count))
    }

    /// The input offset of the oldest of the newest `bytes` buffered bytes,
    /// for uniform stuffing, where each buffered byte is eight bits wide.
    fn buffered_offset(&self, bytes: u32) -> usize {
        (0..bytes).fold(self.bytes.offset, |offset, index| {
            #[expect(clippy::cast_possible_truncation, reason = "one byte of the buffer")]
            let byte = (self.buffer >> (8 * index)) as u8;
            offset.saturating_sub(S::raw_len(byte))
        })
    }

    /// Replays the loads since `start` up to the next unconsumed bit.
    fn locate(&self) -> Location {
        let target = self.consumed();
        let mut cursor = self.bytes.clone();
        cursor.offset = self.start;
        let mut location = Location {
            offset: self.start,
            consumed: 0,
            previous: self.start_previous,
        };
        let mut loaded = 0_u64;
        while loaded < target {
            location.offset = cursor.offset;
            // These bytes loaded before, so loading them again succeeds.
            let Ok((byte, width)) = S::load(&mut cursor, location.previous) else {
                break;
            };
            if loaded + u64::from(width) > target {
                location.consumed = u32::try_from(target - loaded).unwrap_or(0);
                return location;
            }
            loaded += u64::from(width);
            location.offset = cursor.offset;
            location.previous = byte;
        }
        location
    }

    /// Forgets the buffer and continues reading at input `offset`.
    fn restart(&mut self, offset: usize, previous: u8) {
        self.bytes.offset = offset;
        self.count = 0;
        self.loaded = 0;
        self.stopped = None;
        self.previous = previous;
        self.start = offset;
        self.start_previous = previous;
    }

    /// Discards the unread bits of the current byte and returns them, then
    /// lets the stuffing rule consume what follows aligned bit data.
    ///
    /// # Errors
    ///
    /// As [`Stuffing::align`].
    pub fn align(&mut self) -> Result<Padding> {
        if S::UNIFORM {
            let width = ((8 - self.consumed() % 8) % 8) as u32;
            let padding = Padding {
                #[expect(clippy::cast_possible_truncation, reason = "below 8 bits")]
                value: low(self.buffer >> (self.count - width), width) as u8,
                width,
            };
            self.count -= width;
            return Ok(padding);
        }
        let location = self.locate();
        let mut cursor = self.bytes.clone();
        cursor.offset = location.offset;
        let (previous, padding) = if location.consumed > 0 {
            let (byte, width) = S::load(&mut cursor, location.previous)?;
            let width = width - location.consumed;
            #[expect(clippy::cast_possible_truncation, reason = "below 8 bits")]
            let value = low(u64::from(byte), width) as u8;
            (byte, Padding { value, width })
        } else {
            (location.previous, Padding::default())
        };
        S::align(&mut cursor, previous)?;
        self.restart(cursor.offset, 0);
        Ok(padding)
    }

    /// Whether no bits of a loaded byte remain unread.
    pub fn is_aligned(&self) -> bool {
        if S::UNIFORM {
            self.consumed().is_multiple_of(8)
        } else {
            self.locate().consumed == 0
        }
    }

    /// The position of the byte holding the next unread bit.
    pub fn position(&self) -> Position {
        let offset = if S::UNIFORM {
            self.buffered_offset(self.count.div_ceil(8))
        } else {
            self.locate().offset
        };
        self.bytes.position_of(offset)
    }

    /// An upper bound on the unread bits: exact for [`Plain`], larger when
    /// stuffing removes bits that are still unread.
    #[inline]
    pub const fn remaining_bits(&self) -> usize {
        self.bytes
            .remaining()
            .saturating_mul(8)
            .saturating_add(self.count as usize)
    }

    /// The byte cursor at the next unread byte, to continue with
    /// byte-oriented data (markers, segment payloads) once aligned. Bit
    /// reading afterwards resumes wherever the byte cursor stopped.
    ///
    /// # Errors
    ///
    /// An `InvalidArgument` error if bits of a loaded byte are still unread;
    /// call [`BitCursor::align`] first.
    pub fn byte_cursor(&mut self) -> Result<&mut Cursor<'a>> {
        let (offset, previous) = if S::UNIFORM {
            if !self.consumed().is_multiple_of(8) {
                return Err(Error::invalid_argument("bit cursor is not byte-aligned"));
            }
            (self.buffered_offset(self.count / 8), self.previous)
        } else {
            let location = self.locate();
            if location.consumed != 0 {
                return Err(Error::invalid_argument("bit cursor is not byte-aligned"));
            }
            (location.offset, location.previous)
        };
        self.restart(offset, previous);
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
    fn peeking_pads_past_the_end_and_consuming_checks_it() {
        let mut bits = BitCursor::<Plain>::new(Cursor::new(&[0b1011_0000, 0xff]));
        assert_eq!(bits.peek(4), 0b1011);
        assert_eq!(bits.peek(12), 0b1011_0000_1111);
        assert_eq!(bits.peek(40), 0xb0ff_0000);
        bits.consume(4).unwrap();
        assert_eq!(bits.bits(4).unwrap(), 0);
        assert_eq!(bits.position(), Position::Buffer(1));
        assert_eq!(bits.peek(12), 0xff0);
        let error = bits.consume(12).unwrap_err();
        assert_eq!(
            (error.kind, error.position),
            (ErrorKind::Syntax, Position::Buffer(2))
        );
        assert_eq!(
            bits.consume(33).unwrap_err().kind,
            ErrorKind::InvalidArgument
        );
        bits.consume(8).unwrap();
        assert_eq!((bits.peek(8), bits.remaining_bits()), (0, 0));
        bits.consume(0).unwrap();
    }

    #[test]
    fn stuffing_errors_surface_only_when_their_bits_are_needed() {
        let mut bits = BitCursor::<J2kFf7>::new(Cursor::new(&[0xff, 0x80]));
        assert_eq!(bits.peek(16), 0xff00);
        bits.consume(8).unwrap();
        let error = bits.consume(1).unwrap_err();
        assert_eq!(error.message, "invalid JPEG 2000 packet bit stuffing");
        assert_eq!(error.position, Position::Buffer(1));
    }

    #[test]
    fn jpeg_reads_resume_after_a_marker_handled_as_bytes() {
        // Entropy data, an RST0 marker, then the next interval's data.
        let data = [0xab, 0xff, 0x00, 0xff, 0xd0, 0xcd, 0xff, 0x00];
        let mut bits = BitCursor::<JpegFf00>::new(Cursor::new(&data));
        assert_eq!(bits.bits(4).unwrap(), 0xa);
        assert_eq!(bits.peek(16), 0xbff0);
        assert_eq!(bits.bits(12).unwrap(), 0xbff);
        assert!(bits.bit().is_err());
        assert!(bits.align().unwrap().width() == 0 && bits.is_aligned());
        let marker = bits.byte_cursor().unwrap();
        assert_eq!(marker.offset(), 3);
        assert_eq!(marker.take_array::<2>().unwrap(), [0xff, 0xd0]);
        assert_eq!(bits.bits(16).unwrap(), 0xcdff);
        assert_eq!(bits.position(), Position::Buffer(8));
    }

    #[test]
    fn byte_boundaries_survive_the_word_buffer() {
        let data: Vec<u8> = (0..32).collect();
        let mut bits = BitCursor::<Plain>::new(Cursor::new(&data));
        assert_eq!(bits.bits(3).unwrap(), 0);
        assert_eq!(bits.position(), Position::Buffer(0));
        assert!(!bits.is_aligned());
        assert_eq!(bits.align().unwrap().width(), 5);
        assert_eq!(bits.byte_cursor().unwrap().u8().unwrap(), 1);
        assert_eq!(bits.bits(8).unwrap(), 2);
        bits.byte_cursor().unwrap().skip(10).unwrap();
        assert_eq!(bits.bits(8).unwrap(), 13);
        assert_eq!(bits.position(), Position::Buffer(14));
        assert_eq!(bits.remaining_bits(), (32 - 14) * 8);
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
