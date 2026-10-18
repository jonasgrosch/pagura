//! Typed repair and informational diagnostics (ADR 0003).

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

/// Whether a diagnostic reports repaired input or information about valid input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DiagnosticKind {
    /// A bounded correction of malformed input, permitted only in Repair mode.
    Repair(Repair),
    /// A stable tag for information that does not imply malformed input.
    Information(&'static str),
}

impl DiagnosticKind {
    /// The stable `area.item` tag, independently of diagnostic kind.
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Repair(repair) => repair.as_str(),
            Self::Information(tag) => tag,
        }
    }

    /// The repair tag, or `None` for information about valid input.
    pub const fn repair(self) -> Option<Repair> {
        match self {
            Self::Repair(repair) => Some(repair),
            Self::Information(_) => None,
        }
    }
}

/// One reported repair or informational observation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Diagnostic {
    /// Whether this describes repaired input or information.
    pub kind: DiagnosticKind,
    /// A fixed description of the repair or information.
    pub message: &'static str,
    /// Where the problem was detected.
    pub position: Position,
    /// The indirect object concerned, when known.
    pub object: Option<ObjectId>,
}

impl Diagnostic {
    /// Creates a repair diagnostic without position or object context.
    pub const fn new(repair: Repair, message: &'static str) -> Self {
        Self {
            kind: DiagnosticKind::Repair(repair),
            message,
            position: Position::Unknown,
            object: None,
        }
    }

    /// Creates an informational diagnostic, valid in either recovery mode.
    pub const fn information(tag: &'static str, message: &'static str) -> Self {
        Self {
            kind: DiagnosticKind::Information(tag),
            message,
            position: Position::Unknown,
            object: None,
        }
    }

    /// The stable tag, independently of diagnostic kind.
    pub const fn tag(&self) -> &'static str {
        self.kind.tag()
    }

    /// The repair tag, or `None` for informational diagnostics.
    pub const fn repair(&self) -> Option<Repair> {
        self.kind.repair()
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

    /// The error this repair becomes under [`Recovery::Strict`], or `None`
    /// for information that must not reject valid input.
    pub fn to_error(&self) -> Option<Error> {
        let repair = self.repair()?;
        let mut error = Error::syntax(self.message).at(self.position);
        error.repair = Some(repair);
        Some(match self.object {
            Some(object) => error.in_object(object),
            None => error,
        })
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self.kind {
            DiagnosticKind::Repair(_) => "repaired",
            DiagnosticKind::Information(_) => "information",
        };
        write!(f, "{label} [{}]: {}", self.tag(), self.message)?;
        if let Some(object) = self.object {
            write!(f, " (object {object})")?;
        }
        if !matches!(self.position, Position::Unknown) {
            write!(f, " at {}", self.position)?;
        }
        Ok(())
    }
}

/// A diagnostic sink that enforces the [`Recovery`] mode for repairs.
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

    /// Records a diagnostic, rejecting only repairs in strict mode.
    /// Informational diagnostics never authorize or imply a repair.
    ///
    /// # Errors
    ///
    /// Returns a repair diagnostic as a syntax error under [`Recovery::Strict`].
    pub fn repair(&mut self, diagnostic: Diagnostic) -> Result<()> {
        if self.mode == Recovery::Strict
            && let Some(error) = diagnostic.to_error()
        {
            return Err(error);
        }
        self.items.push(diagnostic);
        Ok(())
    }

    /// Diagnostics recorded so far, in discovery order.
    pub fn as_slice(&self) -> &[Diagnostic] {
        &self.items
    }

    /// Whether no diagnostic has been recorded.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Moves the recorded diagnostics out, leaving the sink empty.
    pub fn take(&mut self) -> Vec<Diagnostic> {
        std::mem::take(&mut self.items)
    }

    /// Appends diagnostics already authorized elsewhere, e.g. by a nested parser.
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
    #[test]
    fn information_is_not_a_repair_in_either_recovery_mode() {
        let information = Diagnostic::information("font.substituted", "bundled outlines")
            .in_object(crate::ObjectId::new(7, 0))
            .at(Position::File(12));
        assert_eq!(
            information.kind,
            super::DiagnosticKind::Information("font.substituted")
        );
        assert_eq!(information.tag(), "font.substituted");
        assert_eq!(information.repair(), None);
        assert_eq!(information.to_error(), None);
        assert_eq!(
            information.to_string(),
            "information [font.substituted]: bundled outlines (object 7 0) at file byte 12"
        );
        for mode in [Recovery::Strict, Recovery::Repair] {
            let mut sink = Diagnostics::new(mode);
            sink.repair(information.clone()).unwrap();
            assert_eq!(sink.as_slice(), std::slice::from_ref(&information));
        }
        // Classification is carried by the kind, never inferred from a tag.
        let repair = Diagnostic::new(Repair::new("font.substituted"), "malformed input");
        assert_eq!(repair.kind.tag(), information.kind.tag());
        assert_eq!(repair.kind.repair(), Some(Repair::new("font.substituted")));
        assert!(Diagnostics::new(Recovery::Strict).repair(repair).is_err());
    }
}
