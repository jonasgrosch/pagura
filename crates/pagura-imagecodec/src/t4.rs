//! T.4 and T.6 coding shared by CCITT fax decoding (7.4.6) and JBIG2 MMR
//! regions (T.88 6.2.6): the modified Huffman run lengths, the
//! two-dimensional coding modes and the decoding of one row into the
//! positions of its changing elements.
//!
//! A row is represented by its changing elements (T.4 4.2.1.3.1): the
//! positions where the colour changes, starting from white, in increasing
//! order. Two-dimensional rows are decoded against the previous row's list,
//! so a row costs time in proportion to its changes, not its width.

mod codes;

use codes::Mode;
use pagura_core::bytes::{BitCursor, Cursor, Padding};
use pagura_core::convert::u32_to_usize;
use pagura_core::{Error, Meter, Position, Result};

/// The colour of a run (T.4 4.1.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Color {
    White,
    Black,
}

impl Color {
    const fn other(self) -> Self {
        match self {
            Self::White => Self::Black,
            Self::Black => Self::White,
        }
    }
}

/// What the extension code of the two-dimensional modes means (T.4
/// 4.2.1.3.4, T.6 2.2.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Extensions {
    /// Uncompressed mode is valid but not implemented (CCITT fax).
    Unsupported,
    /// The format forbids extensions (T.88 6.2.6).
    Forbidden,
}

/// An end-of-line code: eleven zeros and a one (T.4 4.1.2).
pub(crate) const EOL: u32 = 0b0000_0000_0001;
/// The length of [`EOL`] in bits.
pub(crate) const EOL_BITS: u32 = 12;

/// A plain most-significant-bit-first reader that charges one unit of
/// decode work for every bit it reads.
pub(crate) struct Bits<'a, 'm> {
    cursor: BitCursor<'a>,
    work: &'m mut Meter,
}

impl<'a, 'm> Bits<'a, 'm> {
    pub(crate) const fn new(bytes: Cursor<'a>, work: &'m mut Meter) -> Self {
        Self {
            cursor: BitCursor::new(bytes),
            work,
        }
    }

    /// Charges `bits` units of work; only a refused charge needs the
    /// position, so it is computed then.
    fn charge(&mut self, bits: u32) -> Result<()> {
        self.work
            .charge(u32_to_usize(bits))
            .map_err(|e| e.at(self.cursor.position()))
    }

    /// Reads one bit.
    pub(crate) fn bit(&mut self) -> Result<bool> {
        self.charge(1)?;
        self.cursor.bit()
    }

    /// Reads `count` bits, at most 32.
    pub(crate) fn bits(&mut self, count: u32) -> Result<u32> {
        self.charge(count)?;
        self.cursor.bits(count)
    }

    /// Whether the next `count` bits are `value`; nothing is consumed.
    pub(crate) fn peek(&self, count: u32, value: u32) -> bool {
        self.cursor
            .clone()
            .bits(count)
            .is_ok_and(|bits| bits == value)
    }

    /// The number of zero bits before the next one bit, if a one bit
    /// follows them. The zeros are charged as work but not consumed.
    pub(crate) fn zeros_before_one(&mut self) -> Result<Option<usize>> {
        let mut ahead = self.cursor.clone();
        let mut zeros = 0;
        loop {
            match ahead.bit() {
                Ok(true) => return Ok(Some(zeros)),
                Ok(false) => {
                    self.work.charge(1).map_err(|e| e.at(ahead.position()))?;
                    zeros += 1;
                }
                Err(_) => return Ok(None),
            }
        }
    }

    /// Whether fewer than eight bits remain and all of them are zero.
    pub(crate) fn only_fill_remains(&self) -> bool {
        let remaining = self.cursor.remaining_bits();
        remaining < 8
            && u32::try_from(remaining)
                .is_ok_and(|count| self.cursor.clone().bits(count).is_ok_and(|v| v == 0))
    }

    /// Unread bits of all remaining bytes.
    pub(crate) const fn remaining(&self) -> usize {
        self.cursor.remaining_bits()
    }

    /// Unread bits of the current byte, 0 to 7.
    pub(crate) const fn unread_in_byte(&self) -> usize {
        // Exact for unstuffed data: whole bytes plus the current one's rest.
        self.cursor.remaining_bits() % 8
    }

    /// Skips to the next byte boundary and returns the bits skipped.
    pub(crate) fn align(&mut self) -> Result<Padding> {
        self.cursor.align()
    }

    /// The offset of the next unread byte, once aligned.
    pub(crate) fn offset(&mut self) -> Result<usize> {
        Ok(self.cursor.byte_cursor()?.offset())
    }

    /// The position of the byte holding the next unread bit.
    pub(crate) fn position(&self) -> Position {
        self.cursor.position()
    }

    /// A copy of the reading position, to return to with [`Bits::reset`].
    pub(crate) fn mark(&self) -> BitCursor<'a> {
        self.cursor.clone()
    }

    /// Returns to a position taken with [`Bits::mark`].
    pub(crate) fn reset(&mut self, mark: BitCursor<'a>) {
        self.cursor = mark;
    }

    /// A syntax error at the byte holding the next unread bit.
    pub(crate) fn error(&self, message: &'static str) -> Error {
        Error::syntax(message).at(self.position())
    }
}

fn syntax_at(message: &'static str, position: Position) -> Error {
    Error::syntax(message).at(position)
}

/// Decodes one run of `color`: make-up codes followed by a terminating code
/// (T.4 4.1.1), at most `remaining` pixels long.
fn run(bits: &mut Bits<'_, '_>, color: Color, remaining: usize) -> Result<usize> {
    let mut total = 0usize;
    loop {
        let at = bits.position();
        let value = codes::run(bits, color)?;
        total = total
            .checked_add(value)
            .filter(|&total| total <= remaining)
            .ok_or(Error::syntax("T.4 run exceeds row").at(at))?;
        if value < 64 {
            return Ok(total);
        }
    }
}

/// The most changing elements a row of `width` pixels may record. Every
/// mode but a vertical one onto a0 advances a0; each records at most two
/// elements (horizontal mode records both of its runs, the second possibly
/// empty at the row end), and only the first mode of a row may start at
/// pixel 0. So valid rows record at most `2 × (width + 1)` elements; the cap
/// stops a run of non-advancing vertical modes.
pub(crate) const fn max_changes(width: usize) -> usize {
    width.saturating_add(1).saturating_mul(2)
}

/// Decodes a one-dimensional row of `width` pixels (T.4 4.1) and appends
/// its changing elements to `changes`.
pub(crate) fn row_1d(
    bits: &mut Bits<'_, '_>,
    width: usize,
    changes: &mut Vec<usize>,
) -> Result<()> {
    let (mut at, mut color) = (0, Color::White);
    while at < width {
        at += run(bits, color, width - at)?;
        changes.push(at);
        color = color.other();
        if changes.len() > max_changes(width) {
            return Err(bits.error("too many T.4 zero-length runs"));
        }
    }
    Ok(())
}

/// Decodes a two-dimensional row of `width` pixels (T.4 4.2.1.3, T.6 2.2)
/// against the changing elements of the `reference` row and appends its own
/// changing elements to `changes`.
pub(crate) fn row_2d(
    bits: &mut Bits<'_, '_>,
    width: usize,
    reference: &[usize],
    changes: &mut Vec<usize>,
    extensions: Extensions,
) -> Result<()> {
    // `at` is a0 and `color` its colour; a0 starts on an imaginary white
    // element before the row, so b1 may be at 0 only there.
    let (mut at, mut color, mut cursor, mut first) = (0, Color::White, 0, true);
    while at < width {
        while reference
            .get(cursor)
            .is_some_and(|&b| b < at || (!first && b == at))
        {
            cursor += 1;
        }
        // Changing elements alternate between white-to-black (even index)
        // and black-to-white; b1 is the next one of the colour opposite to
        // a0's (T.4 4.2.1.3.1).
        let index = if cursor % 2 == usize::from(color == Color::Black) {
            cursor
        } else {
            cursor + 1
        };
        let b1 = reference.get(index).copied().unwrap_or(width);
        let b2 = reference.get(index + 1).copied().unwrap_or(width);
        let at_mode = bits.position();
        match codes::mode(bits)? {
            Mode::Pass => {
                if b2 <= at {
                    return Err(syntax_at("nonadvancing T.6 pass mode", at_mode));
                }
                at = b2;
            }
            Mode::Horizontal => {
                let middle = at + run(bits, color, width - at)?;
                let end = middle + run(bits, color.other(), width - middle)?;
                if end <= at {
                    return Err(syntax_at("nonadvancing T.6 horizontal mode", at_mode));
                }
                changes.push(middle);
                changes.push(end);
                at = end;
            }
            Mode::Vertical(delta) => {
                let end = b1
                    .checked_add_signed(isize::from(delta))
                    .filter(|&end| end >= at && end <= width)
                    .ok_or(syntax_at("T.6 vertical mode outside the row", at_mode))?;
                changes.push(end);
                at = end;
                color = color.other();
            }
            Mode::Extension => {
                return Err(match extensions {
                    Extensions::Unsupported => Error::unsupported(
                        crate::features::CCITT_EXTENSION,
                        "CCITT extension mode is unsupported",
                    ),
                    Extensions::Forbidden => Error::syntax("T.88 forbids T.6 extension codes"),
                }
                .at(at_mode));
            }
        }
        first = false;
        if changes.len() > max_changes(width) {
            return Err(bits.error("too many T.6 changing elements"));
        }
    }
    Ok(())
}

/// Writes a row of `width` pixels with the given `changes` into `row`, one
/// bit per pixel, most significant first: pixels of the colour `ones` become
/// one bits. `row` must be zero.
pub(crate) fn pack_row(row: &mut [u8], changes: &[usize], width: usize, ones: Color) {
    let (mut color, mut from) = (Color::White, 0);
    for to in changes.iter().copied().chain(std::iter::once(width)) {
        if color == ones {
            fill(row, from, to.min(width));
        }
        from = to;
        color = color.other();
    }
}

/// Sets bits `from..to` of a packed row.
fn fill(row: &mut [u8], from: usize, to: usize) {
    for x in from..to {
        if let Some(byte) = row.get_mut(x / 8) {
            *byte |= 0x80 >> (x % 8);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Bits, Color, Extensions, pack_row, row_1d, row_2d};
    use pagura_core::bytes::Cursor;
    use pagura_core::{ErrorKind, Limits, Meter, Position};

    fn work() -> Meter {
        Meter::new(Limits::default().decode_work, "test work budget exceeded")
    }

    #[test]
    fn packs_runs_of_one_colour_as_ones() {
        // White 0, black 1–9, white 10, black 11–12.
        let mut row = [0; 2];
        pack_row(&mut row, &[1, 10, 11], 13, Color::Black);
        assert_eq!(row, [0b0111_1111, 0b1101_1000]);
        let mut row = [0; 2];
        pack_row(&mut row, &[1, 10, 11], 13, Color::White);
        assert_eq!(row, [0b1000_0000, 0b0010_0000]);
    }

    #[test]
    fn decodes_one_and_two_dimensional_rows() {
        // White 2 (0111), black 3 (10), white 3 (1000): eight pixels.
        let mut meter = work();
        let mut bits = Bits::new(Cursor::new(&[0b0111_1010, 0b0000_0000]), &mut meter);
        let mut changes = Vec::new();
        row_1d(&mut bits, 8, &mut changes).unwrap();
        assert_eq!(changes, [2, 5, 8]);
        // V0, V0: the same changing elements again, then V0 to the row end.
        let mut meter = work();
        let mut bits = Bits::new(Cursor::new(&[0b1110_0000]), &mut meter);
        let mut next = Vec::new();
        row_2d(&mut bits, 8, &changes, &mut next, Extensions::Forbidden).unwrap();
        assert_eq!(next, [2, 5, 8]);
        assert_eq!(meter.used(), 3);
    }

    #[test]
    fn extension_codes_follow_the_callers_policy() {
        for (extensions, kind) in [
            (
                Extensions::Unsupported,
                ErrorKind::Unsupported(crate::features::CCITT_EXTENSION),
            ),
            (Extensions::Forbidden, ErrorKind::Syntax),
        ] {
            let mut meter = work();
            let data = [0b0000_0010];
            let mut bits = Bits::new(Cursor::new(&data).with_base(40), &mut meter);
            let error = row_2d(&mut bits, 8, &[], &mut Vec::new(), extensions).unwrap_err();
            assert_eq!((error.kind, error.position), (kind, Position::Buffer(40)));
        }
    }

    #[test]
    fn every_bit_is_charged_before_it_is_read() {
        let mut meter = Meter::new(5, "test work budget exceeded");
        let mut bits = Bits::new(Cursor::new(&[0b0111_1010]), &mut meter);
        let error = row_1d(&mut bits, 8, &mut Vec::new()).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit);
        assert_eq!(meter.used(), 5);
    }
}
