//! Font-buffer cursors and positioned errors.
//!
//! Tables are read through `pagura_core::bytes::Cursor`, positioned in the
//! program bytes. Truncation is a `Syntax` error and offset overflow a
//! `Limit` error, as for every binary format (CONTRIBUTING, Conventions 6).
use pagura_core::bytes::Cursor;
use pagura_core::{Error, Feature, Position, Result};
use std::ops::Range;

const TRUNCATED: &str = "truncated font data";

/// A cursor over the whole program.
pub(crate) const fn cursor(data: &[u8]) -> Cursor<'_> {
    Cursor::new(data).with_message(TRUNCATED)
}

/// A cursor over `range` of the program, positioned in the program, or a
/// `Syntax` error with `message` if the range is outside it.
pub(crate) fn slice<'a>(
    data: &'a [u8],
    range: Range<usize>,
    message: &'static str,
) -> Result<Cursor<'a>> {
    let start = range.start;
    let bytes = data
        .get(range)
        .ok_or_else(|| syntax(Position::Buffer(start), message))?;
    Ok(cursor(bytes).with_base(start))
}

/// The program offset of the cursor's next byte. Font cursors report
/// buffer positions, so this is their base plus their offset.
pub(crate) fn here(cursor: &Cursor<'_>) -> usize {
    if let Position::Buffer(offset) = cursor.position() {
        offset
    } else {
        cursor.offset()
    }
}

pub(crate) const fn syntax(at: Position, message: &'static str) -> Error {
    Error::syntax(message).at(at)
}

pub(crate) const fn limit(at: Position, message: &'static str) -> Error {
    Error::limit(message).at(at)
}

pub(crate) const fn unsupported(feature: Feature, at: Position, message: &'static str) -> Error {
    Error::unsupported(feature, message).at(at)
}

/// The glyph at `index`, or a `Limit` error if the index exceeds 32 bits.
pub(crate) fn glyph_id(index: usize, at: Position) -> Result<crate::GlyphId> {
    u32::try_from(index)
        .map(crate::GlyphId)
        .map_err(|_| limit(at, "glyph index overflow"))
}

#[cfg(test)]
mod tests {
    use super::{cursor, slice};
    use pagura_core::{ErrorKind, Position};

    #[test]
    fn slices_keep_program_positions_and_reject_ranges_outside_it() {
        let data = [0, 1, 2, 3, 4, 5];
        let mut table = slice(&data, 2..5, "table outside font").unwrap();
        assert_eq!(table.u16().unwrap(), 0x0203);
        let error = table.u16().unwrap_err();
        assert_eq!(
            (error.kind, error.position),
            (ErrorKind::Syntax, Position::Buffer(4))
        );
        assert_eq!(error.message, "truncated font data");
        let error = slice(&data, 4..9, "table outside font").unwrap_err();
        assert_eq!(
            (error.kind, error.position),
            (ErrorKind::Syntax, Position::Buffer(4))
        );
        assert_eq!(
            cursor(&data).cursor_at(7).unwrap_err().kind,
            ErrorKind::Syntax
        );
    }
}
