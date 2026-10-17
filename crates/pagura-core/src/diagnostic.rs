//! Reported repairs of malformed input (ADR 0003).

use crate::{Error, ObjectId, Position, Result};
use std::fmt;

/// Whether malformed input may be repaired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum Recovery {
    /// Every deviation from the standard is an error. Used for validation
    /// and before incremental saving.
    Strict,
    /// Bounded, documented repairs are applied and each one is reported as a
    /// [`Diagnostic`]. Used for viewing.
    #[default]
    Repair,
}

/// A stable, machine-readable tag for a kind of repair, `area.item`, for
/// example `xref.rebuilt` or `stream.length-corrected`. Each crate defines
/// its own constants; published tags must not change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Repair(&'static str);

impl Repair {
    /// Creates a tag. Follow the `area.item` convention.
    pub const fn new(tag: &'static str) -> Self {
        Self(tag)
    }

    /// The tag text.
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for Repair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// One repair that was applied to malformed input.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Diagnostic {
    /// What kind of repair.
    pub repair: Repair,
    /// The violated requirement, as a fixed description.
    pub message: &'static str,
    /// Where the problem was detected.
    pub position: Position,
    /// The indirect object concerned, when known.
    pub object: Option<ObjectId>,
}

impl Diagnostic {
    /// Creates a diagnostic without position or object context.
    pub const fn new(repair: Repair, message: &'static str) -> Self {
        Self {
            repair,
            message,
            position: Position::Unknown,
            object: None,
        }
    }

    /// Sets the position.
    #[must_use]
    pub const fn at(mut self, position: Position) -> Self {
        self.position = position;
        self
    }

    /// Sets the object context.
    #[must_use]
    pub const fn in_object(mut self, id: ObjectId) -> Self {
        self.object = Some(id);
        self
    }

    /// The error this repair becomes under [`Recovery::Strict`].
    pub fn to_error(&self) -> Error {
        let mut error = Error::syntax(self.message).at(self.position);
        error.repair = Some(self.repair);
        match self.object {
            Some(object) => error.in_object(object),
            None => error,
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "repaired [{}]: {}", self.repair, self.message)?;
        if let Some(object) = self.object {
            write!(f, " (object {object})")?;
        }
        if !matches!(self.position, Position::Unknown) {
            write!(f, " at {}", self.position)?;
        }
        Ok(())
    }
}

/// A sink for repairs that enforces the [`Recovery`] mode.
///
/// Code that is about to repair input calls [`Diagnostics::repair`] first;
/// in strict mode that returns the error instead of permitting the repair.
#[derive(Debug, Clone, Default)]
pub struct Diagnostics {
    mode: Recovery,
    items: Vec<Diagnostic>,
}

impl Diagnostics {
    /// Creates an empty sink for `mode`.
    pub const fn new(mode: Recovery) -> Self {
        Self {
            mode,
            items: Vec::new(),
        }
    }

    /// The recovery mode.
    pub const fn mode(&self) -> Recovery {
        self.mode
    }

    /// Records a repair, or rejects it in strict mode.
    ///
    /// # Errors
    ///
    /// Returns the diagnostic as a syntax error under [`Recovery::Strict`].
    pub fn repair(&mut self, diagnostic: Diagnostic) -> Result<()> {
        match self.mode {
            Recovery::Strict => Err(diagnostic.to_error()),
            Recovery::Repair => {
                self.items.push(diagnostic);
                Ok(())
            }
        }
    }

    /// Repairs recorded so far, in the order they were applied.
    pub fn as_slice(&self) -> &[Diagnostic] {
        &self.items
    }

    /// Whether no repair was applied.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Moves the recorded repairs out, leaving the sink empty.
    pub fn take(&mut self) -> Vec<Diagnostic> {
        std::mem::take(&mut self.items)
    }

    /// Appends repairs recorded elsewhere, e.g. by a nested parser.
    pub fn extend(&mut self, items: impl IntoIterator<Item = Diagnostic>) {
        self.items.extend(items);
    }
}

#[cfg(test)]
mod tests {
    use super::{Diagnostic, Diagnostics, Recovery, Repair};
    use crate::{ErrorKind, Position};

    const TRAILING: Repair = Repair::new("file.trailing-bytes");

    #[test]
    fn strict_mode_turns_repairs_into_errors() {
        let diagnostic = Diagnostic::new(TRAILING, "bytes after %%EOF").at(Position::File(7));
        let mut strict = Diagnostics::new(Recovery::Strict);
        let error = strict.repair(diagnostic.clone()).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax);
        assert_eq!(error.position, Position::File(7));
        assert_eq!(error.repair, Some(TRAILING));
        let mut repair = Diagnostics::new(Recovery::Repair);
        repair.repair(diagnostic.clone()).unwrap();
        assert_eq!(repair.as_slice(), [diagnostic]);
    }
}
