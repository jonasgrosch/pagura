//! Checked reading of big-endian binary formats: font tables, image codec
//! segments, ICC profiles and cross-reference stream rows.
//!
//! Every read is bounds-checked and returns a positioned [`Error`]. Reading
//! past the end of the data is a `Syntax` error (the input is truncated);
//! an offset or length whose arithmetic overflows `usize` is a `Limit` error
//! (the input asks for more than the address space).

mod bits;

pub use bits::{BitCursor, J2kFf7, JpegFf00, Padding, Plain, Stuffing};

use crate::syntax::Origin;
use crate::{Error, Position, Result};

/// A big-endian reader over a byte slice that knows where it is.
///
/// Positions are reported as `origin.position(base + offset)`: by default a
/// [`Position::Buffer`] offset from the start of the slice. A parser of one
/// table inside a larger buffer sets [`Cursor::with_base`] so errors carry
/// offsets in the larger buffer; a reader of a stream's decoded data can set
/// [`Cursor::with_origin`] to report stream positions directly.
///
/// ```
/// use pagura_core::bytes::Cursor;
/// use pagura_core::{ErrorKind, Position};
///
/// let mut cursor = Cursor::new(&[0x00, 0x01, 0xff, 0xfe, 0x40, 0x00]).with_base(100);
/// assert_eq!(cursor.u16()?, 1);
/// assert_eq!(cursor.i16()?, -2);
/// assert_eq!(cursor.f2dot14()?, 1.0);
/// let error = cursor.u8().unwrap_err();
/// assert_eq!((error.kind, error.position), (ErrorKind::Syntax, Position::Buffer(106)));
/// # Ok::<(), pagura_core::Error>(())
/// ```
#[derive(Debug, Clone)]
pub struct Cursor<'a> {
    data: &'a [u8],
    offset: usize,
    origin: Origin,
    base: usize,
    truncated: &'static str,
}

impl<'a> Cursor<'a> {
    /// A cursor at the start of `data`, reporting buffer offsets.
    pub const fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            offset: 0,
            origin: Origin::Buffer,
            base: 0,
            truncated: "truncated binary data",
        }
    }

    /// Reports positions as if `data` started at `base` in its input.
    #[must_use]
    pub const fn with_base(mut self, base: usize) -> Self {
        self.base = base;
        self
    }

    /// Reports positions in `origin`'s input instead of a standalone buffer.
    #[must_use]
    pub const fn with_origin(mut self, origin: Origin) -> Self {
        self.origin = origin;
        self
    }

    /// Uses `message` for truncation errors, naming the format, for example
    /// `"truncated font data"`.
    #[must_use]
    pub const fn with_message(mut self, message: &'static str) -> Self {
        self.truncated = message;
        self
    }

    /// The whole slice, including bytes already read.
    pub const fn data(&self) -> &'a [u8] {
        self.data
    }

    /// The unread bytes, without consuming them.
    pub fn rest(&self) -> &'a [u8] {
        self.data.get(self.offset..).unwrap_or_default()
    }

    /// The offset of the next unread byte from the start of the slice.
    pub const fn offset(&self) -> usize {
        self.offset
    }

    /// The position of the next unread byte.
    pub const fn position(&self) -> Position {
        self.position_of(self.offset)
    }

    const fn position_of(&self, offset: usize) -> Position {
        self.origin.position(self.base.saturating_add(offset))
    }

    /// The number of unread bytes.
    pub const fn remaining(&self) -> usize {
        self.data.len() - self.offset
    }

    /// Whether every byte has been read.
    pub const fn at_end(&self) -> bool {
        self.offset == self.data.len()
    }

    /// A `Syntax` error with `message` at the next unread byte, for format
    /// errors the caller detects.
    pub const fn error(&self, message: &'static str) -> Error {
        Error::syntax(message).at(self.position())
    }

    /// Moves to `offset` from the start of the slice.
    ///
    /// # Errors
    ///
    /// A `Syntax` error if `offset` is beyond the end of the data.
    pub fn seek(&mut self, offset: usize) -> Result<()> {
        if offset > self.data.len() {
            return Err(Error::syntax(self.truncated).at(self.position_of(offset)));
        }
        self.offset = offset;
        Ok(())
    }

    /// Reads the next `count` bytes.
    ///
    /// # Errors
    ///
    /// A `Syntax` error if fewer than `count` bytes remain, or a `Limit`
    /// error if the end offset overflows.
    pub fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(count)
            .ok_or_else(|| Error::limit("binary offset overflow").at(self.position()))?;
        let bytes = self
            .data
            .get(self.offset..end)
            .ok_or_else(|| Error::syntax(self.truncated).at(self.position()))?;
        self.offset = end;
        Ok(bytes)
    }

    /// Reads the next `N` bytes as an array.
    ///
    /// # Errors
    ///
    /// As [`Cursor::take`].
    pub fn take_array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let (array, _) = self
            .rest()
            .split_first_chunk::<N>()
            .ok_or_else(|| Error::syntax(self.truncated).at(self.position()))?;
        self.offset += N;
        Ok(*array)
    }

    /// Reads every remaining byte.
    pub fn take_rest(&mut self) -> &'a [u8] {
        let rest = self.rest();
        self.offset = self.data.len();
        rest
    }

    /// Skips the next `count` bytes.
    ///
    /// # Errors
    ///
    /// As [`Cursor::take`].
    pub fn skip(&mut self, count: usize) -> Result<()> {
        self.take(count).map(|_| ())
    }

    /// Takes the next `count` bytes as a cursor of their own, for a segment
    /// or table body. Its positions stay in this cursor's input and it uses
    /// the same truncation message.
    ///
    /// # Errors
    ///
    /// As [`Cursor::take`].
    pub fn take_cursor(&mut self, count: usize) -> Result<Self> {
        let start = self.offset;
        let data = self.take(count)?;
        Ok(self.child(data, start))
    }

    /// A cursor over the bytes from `offset` (from the start of this slice)
    /// to the end, for a table found through an offset field. This cursor
    /// does not move.
    ///
    /// # Errors
    ///
    /// A `Syntax` error if `offset` is beyond the end of the data.
    pub fn cursor_at(&self, offset: usize) -> Result<Self> {
        let data = self
            .data
            .get(offset..)
            .ok_or_else(|| Error::syntax(self.truncated).at(self.position_of(offset)))?;
        Ok(self.child(data, offset))
    }

    const fn child(&self, data: &'a [u8], start: usize) -> Self {
        Self {
            data,
            offset: 0,
            origin: self.origin,
            base: self.base.saturating_add(start),
            truncated: self.truncated,
        }
    }

    /// Reads an unsigned byte.
    ///
    /// # Errors
    ///
    /// A `Syntax` error at the end of the data.
    pub fn u8(&mut self) -> Result<u8> {
        self.take_array().map(u8::from_be_bytes)
    }

    /// Reads a signed byte.
    ///
    /// # Errors
    ///
    /// As [`Cursor::u8`].
    pub fn i8(&mut self) -> Result<i8> {
        self.take_array().map(i8::from_be_bytes)
    }

    /// Reads a big-endian `u16`.
    ///
    /// # Errors
    ///
    /// A `Syntax` error if fewer than 2 bytes remain.
    pub fn u16(&mut self) -> Result<u16> {
        self.take_array().map(u16::from_be_bytes)
    }

    /// Reads a big-endian `i16`.
    ///
    /// # Errors
    ///
    /// As [`Cursor::u16`].
    pub fn i16(&mut self) -> Result<i16> {
        self.take_array().map(i16::from_be_bytes)
    }

    /// Reads a big-endian `u32`.
    ///
    /// # Errors
    ///
    /// A `Syntax` error if fewer than 4 bytes remain.
    pub fn u32(&mut self) -> Result<u32> {
        self.take_array().map(u32::from_be_bytes)
    }

    /// Reads a big-endian `i32`.
    ///
    /// # Errors
    ///
    /// As [`Cursor::u32`].
    pub fn i32(&mut self) -> Result<i32> {
        self.take_array().map(i32::from_be_bytes)
    }

    /// Reads a big-endian unsigned integer of `width` bytes, as in
    /// cross-reference stream fields (clause 7.5.8.2) or CFF offsets. A
    /// width of zero reads nothing and yields zero.
    ///
    /// # Errors
    ///
    /// A `Limit` error if `width` exceeds 8 bytes, otherwise as
    /// [`Cursor::take`].
    pub fn uint(&mut self, width: usize) -> Result<u64> {
        if width > 8 {
            return Err(Error::limit("binary integer wider than 64 bits").at(self.position()));
        }
        let bytes = self.take(width)?;
        Ok(bytes
            .iter()
            .fold(0, |value, &byte| (value << 8) | u64::from(byte)))
    }

    /// Reads a signed 16.16 fixed-point number (OpenType `Fixed`).
    ///
    /// # Errors
    ///
    /// As [`Cursor::u32`].
    pub fn fixed(&mut self) -> Result<f64> {
        self.i32().map(|value| f64::from(value) / 65_536.0)
    }

    /// Reads a signed 2.14 fixed-point number (OpenType `F2DOT14`).
    ///
    /// # Errors
    ///
    /// As [`Cursor::u16`].
    pub fn f2dot14(&mut self) -> Result<f64> {
        self.i16().map(|value| f64::from(value) / 16_384.0)
    }
}

#[cfg(test)]
mod tests {
    use super::Cursor;
    use crate::syntax::Origin;
    use crate::{ErrorKind, ObjectId, Position};

    #[test]
    fn integers_are_big_endian_and_fixed_point_is_signed() {
        let data = [
            0x80, 0xff, 0x12, 0x34, 0xfe, 0xdc, 0x01, 0x02, 0x03, 0x04, 0xff, 0xff, 0xff, 0xfe,
            0x00, 0x01, 0x80, 0x00, 0xc0, 0x00,
        ];
        let mut cursor = Cursor::new(&data);
        assert_eq!(cursor.u8().unwrap(), 0x80);
        assert_eq!(cursor.i8().unwrap(), -1);
        assert_eq!(cursor.u16().unwrap(), 0x1234);
        assert_eq!(cursor.i16().unwrap(), -292);
        assert_eq!(cursor.u32().unwrap(), 0x0102_0304);
        assert_eq!(cursor.i32().unwrap(), -2);
        assert_eq!(cursor.fixed().unwrap(), 1.5);
        assert_eq!(cursor.f2dot14().unwrap(), -1.0);
        assert!(cursor.at_end());
        let mut cursor = Cursor::new(&data);
        assert_eq!(cursor.uint(0).unwrap(), 0);
        assert_eq!(cursor.uint(3).unwrap(), 0x80_ff12);
        assert_eq!(cursor.uint(8).unwrap(), 0x34fe_dc01_0203_04ff);
        assert_eq!(cursor.uint(9).unwrap_err().kind, ErrorKind::Limit);
        assert_eq!(cursor.take_array::<2>().unwrap(), [0xff, 0xff]);
        assert_eq!(cursor.remaining(), 7);
    }

    #[test]
    fn truncation_is_syntax_and_overflow_is_limit_without_moving() {
        let mut cursor = Cursor::new(b"abc").with_message("truncated test data");
        cursor.skip(2).unwrap();
        let error = cursor.u16().unwrap_err();
        assert_eq!(
            (error.kind, error.message, error.position),
            (
                ErrorKind::Syntax,
                "truncated test data",
                Position::Buffer(2)
            )
        );
        assert_eq!(cursor.offset(), 2);
        let error = cursor.take(usize::MAX).unwrap_err();
        assert_eq!(
            (error.kind, error.position),
            (ErrorKind::Limit, Position::Buffer(2))
        );
        assert_eq!(cursor.seek(4).unwrap_err().position, Position::Buffer(4));
        cursor.seek(3).unwrap();
        assert_eq!(cursor.take_rest(), b"");
        assert_eq!(cursor.take_array::<0>().unwrap(), []);
    }

    #[test]
    fn sub_cursors_report_positions_in_the_parent_input() {
        let stream = ObjectId::new(6, 0);
        let mut cursor = Cursor::new(b"0123456789")
            .with_origin(Origin::Stream(stream))
            .with_base(100);
        cursor.skip(2).unwrap();
        let mut body = cursor.take_cursor(4).unwrap();
        assert_eq!(cursor.offset(), 6);
        assert_eq!(body.take_rest(), b"2345");
        assert_eq!(
            body.u8().unwrap_err().position,
            Position::Stream {
                object: stream,
                offset: 106
            }
        );
        let table = cursor.cursor_at(8).unwrap();
        assert_eq!((table.rest(), table.offset()), (b"89".as_slice(), 0));
        assert_eq!(
            table.position(),
            Position::Stream {
                object: stream,
                offset: 108
            }
        );
        assert_eq!(cursor.offset(), 6);
        assert!(cursor.cursor_at(11).is_err());
        assert_eq!(cursor.cursor_at(10).unwrap().remaining(), 0);
        assert_eq!(cursor.error("bad").kind, ErrorKind::Syntax);
        assert_eq!(cursor.data().len(), 10);
    }
}
