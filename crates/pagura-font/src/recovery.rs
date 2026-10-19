//! Which font failures reported recovery may replace (ADR 0003).
use pagura_core::{Error, ErrorKind};

/// Whether `error` is malformed or unsupported input in one glyph or
/// table, which a reported repair may replace. Exhausted budgets, offset
/// overflow and rejected repairs are never replaced.
pub(crate) fn glyph_error(error: &Error) -> bool {
    error.repair.is_none() && matches!(error.kind, ErrorKind::Syntax | ErrorKind::Unsupported(_))
}
