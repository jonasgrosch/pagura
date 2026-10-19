//! Positioned errors and repairs shared by the decoders. Offsets are in the
//! decoder's input, as [`Position::Buffer`].
use crate::repairs;
use pagura_core::syntax::is_whitespace;
use pagura_core::{Diagnostic, Diagnostics, Error, Position, Repair, Result};

pub(crate) fn syntax(message: &'static str, offset: usize) -> Error {
    Error::syntax(message).at(Position::Buffer(offset))
}

pub(crate) fn repair(
    diagnostics: &mut Diagnostics,
    tag: Repair,
    message: &'static str,
    offset: usize,
) -> Result<()> {
    diagnostics.repair(Diagnostic::new(tag, message).at(Position::Buffer(offset)))
}

/// What may follow a filter's end marker without a repair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Trailer {
    /// PDF whitespace, which the ASCII filters ignore anywhere (7.4.2, 7.4.3).
    Whitespace,
    /// Nothing: any byte after a binary filter's end, even zero, is reported.
    Nothing,
}

/// Reports [`repairs::TRAILING_DATA`] if `input` continues after `end` with
/// anything `trailer` does not admit.
pub(crate) fn trailing(
    input: &[u8],
    end: usize,
    trailer: Trailer,
    diagnostics: &mut Diagnostics,
) -> Result<()> {
    if let Some(bytes) = input.get(end..)
        && let Some(index) = bytes
            .iter()
            .position(|&byte| trailer == Trailer::Nothing || !is_whitespace(byte))
    {
        repair(
            diagnostics,
            repairs::TRAILING_DATA,
            "data after filter end marker",
            end + index,
        )?;
    }
    Ok(())
}
