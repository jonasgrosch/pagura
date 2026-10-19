//! Font-buffer cursors and positioned errors.
//!
//! Tables are read through `pagura_core::bytes::Cursor`, positioned in the
//! program bytes. Truncation is a `Syntax` error and offset overflow a
//! `Limit` error, as for every binary format (CONTRIBUTING, Conventions 6).
use pagura_core::bytes::Cursor;
use pagura_core::{Error, Position, Result};
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

/// A `Syntax` error at buffer offset `at`, for parsers whose errors are
/// not yet positioned through their cursors.
pub(crate) fn bad(at: usize, message: &'static str) -> Error {
    syntax(Position::Buffer(at), message)
}

pub(crate) const fn syntax(at: Position, message: &'static str) -> Error {
    Error::syntax(message).at(at)
}

pub(crate) const fn limit(at: Position, message: &'static str) -> Error {
    Error::limit(message).at(at)
}

/// Checks that every point of an outline is finite: arithmetic beyond
/// `f64` range is malformed glyph data.
pub(crate) fn finite_path(path: &pagura_raster::Path) -> Result<()> {
    use pagura_raster::Segment;
    for segment in path.segments() {
        let finite = match *segment {
            Segment::MoveTo(p) | Segment::LineTo(p) => p.is_finite(),
            Segment::CubicTo(a, b, c) => a.is_finite() && b.is_finite() && c.is_finite(),
            Segment::Close => true,
        };
        if !finite {
            return Err(syntax(Position::Unknown, "nonfinite glyph outline"));
        }
    }
    Ok(())
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
