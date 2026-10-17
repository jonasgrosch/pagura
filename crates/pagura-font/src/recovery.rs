//! Font recovery admission and bounded diagnostic coalescing (ADR 0003).
use pagura_core::{Diagnostic, Diagnostics, Error, ErrorKind, Position, Recovery, Repair, Result};

pub(crate) fn repair(
    diagnostics: &mut Diagnostics,
    tag: Repair,
    message: &'static str,
    position: Position,
) -> Result<()> {
    // Each operation has a fresh sink. Retain the first occurrence of each
    // font repair kind, rather than allocating once per damaged point or code.
    if diagnostics.mode() == Recovery::Strict
        || !diagnostics.as_slice().iter().any(|d| d.repair == tag)
    {
        diagnostics.repair(Diagnostic::new(tag, message).at(position))?;
    }
    Ok(())
}

pub(crate) fn glyph_error(error: &Error) -> bool {
    error.repair.is_none() && matches!(error.kind, ErrorKind::Syntax | ErrorKind::Unsupported(_))
}

pub(crate) fn legacy_error(mut error: Error) -> Error {
    // Preserve the original strict API's positioned syntax-error contract.
    if error.kind == ErrorKind::Limit {
        error.kind = ErrorKind::Syntax;
    }
    error
}
