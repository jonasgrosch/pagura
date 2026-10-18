//! Positioned errors and repairs shared by the decoders.
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

pub(crate) fn trailing(
    input: &[u8],
    end: usize,
    ascii: bool,
    diagnostics: &mut Diagnostics,
) -> Result<()> {
    if let Some(bytes) = input.get(end..)
        && let Some(index) = bytes
            .iter()
            .position(|&byte| !ascii || !is_whitespace(byte))
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
