//! Typed repair and informational diagnostics (ADR 0003).

use crate::{Error, ObjectId, Position, Result};
use std::fmt;

/// Whether malformed input may be repaired.
///
/// New modes may be added, so matches outside this crate need a wildcard arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
#[non_exhaustive]
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
/// its own constants in a `repairs` module (see `CONTRIBUTING.md`);
/// published tags must not change.
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
///
/// New kinds may be added, so matches outside this crate need a wildcard arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
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
///
/// The builders keep the innermost context, exactly like [`Error`]'s: a
/// position or object recorded closer to the fault is never overwritten.
/// Fields stay public for reading; the type is `#[non_exhaustive]` so other
/// crates build it through [`Diagnostic::new`] or [`Diagnostic::information`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
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

    /// Sets the position unless one is already known. A
    /// [`Position::Stream`] also records its stream as the object context,
    /// unless an inner object is already recorded.
    #[must_use]
    pub const fn at(mut self, position: Position) -> Self {
        if matches!(self.position, Position::Unknown) {
            self.position = position;
            self = self.in_object_opt(position.object());
        }
        self
    }

    /// Sets the object context unless an inner object is already recorded.
    #[must_use]
    pub const fn in_object(mut self, id: ObjectId) -> Self {
        if self.object.is_none() {
            self.object = Some(id);
        }
        self
    }

    /// [`Diagnostic::in_object`] for an object that may not be known.
    #[must_use]
    pub const fn in_object_opt(self, id: Option<ObjectId>) -> Self {
        match id {
            Some(id) => self.in_object(id),
            None => self,
        }
    }

    /// Places a buffer-relative position in the input the buffer came from;
    /// see [`Position::within`]. A resulting stream position also records the
    /// stream as the object context, unless an inner object is recorded.
    #[must_use]
    pub const fn within(mut self, start: Position) -> Self {
        self.position = self.position.within(start);
        let object = self.position.object();
        self.in_object_opt(object)
    }

    /// The error this repair becomes under [`Recovery::Strict`], or `None`
    /// for information that must not reject valid input.
    pub const fn to_error(&self) -> Option<Error> {
        match self.repair() {
            Some(repair) => Some(
                Error::syntax(self.message)
                    .at(self.position)
                    .in_object_opt(self.object)
                    .with_repair(repair),
            ),
            None => None,
        }
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
/// Every way of adding a diagnostic goes through that check, so a strict
/// sink never holds a repair.
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

    /// Records a diagnostic unless one of the same kind (the same repair or
    /// information tag) is already recorded; strict mode still rejects every
    /// repair.
    ///
    /// Use this where one damaged structure can repeat the same deviation
    /// many times (every glyph, code or row): the first occurrence is kept
    /// with its position, and the sink does not grow per occurrence.
    ///
    /// # Errors
    ///
    /// As [`Diagnostics::repair`].
    pub fn repair_once(&mut self, diagnostic: Diagnostic) -> Result<()> {
        if self.items.iter().any(|item| item.kind == diagnostic.kind) {
            return Ok(());
        }
        self.repair(diagnostic)
    }

    /// Records diagnostics collected by a nested sink or parser, each through
    /// [`Diagnostics::repair`], so this sink's mode applies to them too.
    ///
    /// # Errors
    ///
    /// Under [`Recovery::Strict`], returns the first repair as a syntax error.
    pub fn absorb(&mut self, items: impl IntoIterator<Item = Diagnostic>) -> Result<()> {
        items.into_iter().try_for_each(|item| self.repair(item))
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
}

#[cfg(test)]
mod tests {
    use super::{Diagnostic, DiagnosticKind, Diagnostics, Recovery, Repair};
    use crate::{ErrorKind, ObjectId, Position};

    const TRAILING: Repair = Repair::new("file.trailing-bytes");
    const OTHER: Repair = Repair::new("file.other");

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
            .in_object(ObjectId::new(7, 0))
            .at(Position::File(12));
        assert_eq!(
            information.kind,
            DiagnosticKind::Information("font.substituted")
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

    #[test]
    fn builders_keep_the_innermost_context_like_errors() {
        let stream = ObjectId::new(3, 0);
        let diagnostic = Diagnostic::new(TRAILING, "bytes after %%EOF")
            .at(Position::Buffer(4))
            .within(Position::Stream {
                object: stream,
                offset: 10,
            })
            .at(Position::File(1))
            .in_object(ObjectId::new(8, 0));
        assert_eq!(
            diagnostic.position,
            Position::Stream {
                object: stream,
                offset: 14
            }
        );
        assert_eq!(diagnostic.object, Some(stream));
        let error = diagnostic.to_error().unwrap();
        assert_eq!(
            (error.position, error.object, error.repair),
            (diagnostic.position, Some(stream), Some(TRAILING))
        );
        let placed = Diagnostic::new(TRAILING, "bytes after %%EOF")
            .in_object_opt(None)
            .in_object_opt(Some(ObjectId::new(5, 0)))
            .at(Position::File(2))
            .at(Position::File(9));
        assert_eq!(
            (placed.position, placed.object),
            (Position::File(2), Some(ObjectId::new(5, 0)))
        );
    }

    #[test]
    fn repair_once_keeps_the_first_occurrence_per_kind_but_strict_rejects_each() {
        let mut sink = Diagnostics::new(Recovery::Repair);
        for offset in 0..100 {
            sink.repair_once(Diagnostic::new(TRAILING, "first").at(Position::Buffer(offset)))
                .unwrap();
        }
        sink.repair_once(Diagnostic::new(OTHER, "second")).unwrap();
        sink.repair_once(Diagnostic::information("file.note", "third"))
            .unwrap();
        sink.repair_once(Diagnostic::information("file.note", "again"))
            .unwrap();
        let tags: Vec<_> = sink.as_slice().iter().map(Diagnostic::tag).collect();
        assert_eq!(tags, ["file.trailing-bytes", "file.other", "file.note"]);
        assert_eq!(sink.as_slice()[0].position, Position::Buffer(0));
        let mut strict = Diagnostics::new(Recovery::Strict);
        for _ in 0..2 {
            let error = strict
                .repair_once(Diagnostic::new(TRAILING, "first"))
                .unwrap_err();
            assert_eq!(error.repair, Some(TRAILING));
        }
        strict
            .repair_once(Diagnostic::information("file.note", "kept"))
            .unwrap();
        assert_eq!(strict.as_slice().len(), 1);
    }

    #[test]
    fn absorbing_respects_the_receiving_mode() {
        let mut nested = Diagnostics::new(Recovery::Repair);
        nested
            .repair(Diagnostic::information("file.note", "nested"))
            .unwrap();
        nested.repair(Diagnostic::new(TRAILING, "nested")).unwrap();
        nested.repair(Diagnostic::new(OTHER, "nested")).unwrap();
        let mut repair = Diagnostics::new(Recovery::Repair);
        repair.absorb(nested.as_slice().to_vec()).unwrap();
        assert_eq!(repair.as_slice(), nested.as_slice());
        let mut strict = Diagnostics::new(Recovery::Strict);
        let error = strict.absorb(nested.take()).unwrap_err();
        assert_eq!(error.repair, Some(TRAILING));
        let kinds: Vec<_> = strict.as_slice().iter().map(|item| item.kind).collect();
        assert_eq!(kinds, [DiagnosticKind::Information("file.note")]);
    }
}
