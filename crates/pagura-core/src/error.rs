//! Structured errors shared by every Pagura crate.

use crate::{ObjectId, Repair};
use std::fmt;

/// A stable, machine-readable tag for an unimplemented PDF capability.
///
/// Tags are `area.item`, for example `filter.DCTDecode`, `operator.Tj`,
/// `colorspace.ICCBased` or `security.PubSec`. Areas are lowercase; items use
/// the PDF spelling. Scoreboards and API callers group failures by tag, so a
/// tag must not change once published. Each crate defines its own constants
/// in a `features` module (see `CONTRIBUTING.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Feature(&'static str);

impl Feature {
    /// Creates a tag. Follow the `area.item` convention.
    pub const fn new(tag: &'static str) -> Self {
        Self(tag)
    }

    /// The tag text.
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for Feature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// The category of an error. Callers branch on this, never on message text.
///
/// New kinds may be added, so matches outside this crate need a wildcard arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorKind {
    /// The input violates PDF syntax or structure and was not repaired.
    Syntax,
    /// A resource budget from [`crate::Limits`] would be exceeded.
    Limit,
    /// The input is valid but needs a capability Pagura does not implement.
    Unsupported(Feature),
    /// A specifically requested object does not exist.
    MissingObject,
    /// A reference graph that must be acyclic contains a cycle.
    Cycle,
    /// The document needs a password that was not supplied or is wrong.
    Password,
    /// The caller passed an invalid argument, independent of any document.
    InvalidArgument,
}

impl ErrorKind {
    /// A stable machine-readable name, e.g. for JSON reports. The feature
    /// tag of `Unsupported` is available separately via [`Error::feature`].
    pub const fn name(self) -> &'static str {
        match self {
            Self::Syntax => "syntax",
            Self::Limit => "limit",
            Self::Unsupported(_) => "unsupported",
            Self::MissingObject => "missing-object",
            Self::Cycle => "cycle",
            Self::Password => "password",
            Self::InvalidArgument => "invalid-argument",
        }
    }
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax => f.write_str("syntax error"),
            Self::Limit => f.write_str("limit exceeded"),
            Self::Unsupported(feature) => write!(f, "unsupported [{feature}]"),
            Self::MissingObject => f.write_str("missing object"),
            Self::Cycle => f.write_str("cycle"),
            Self::Password => f.write_str("password required"),
            Self::InvalidArgument => f.write_str("invalid argument"),
        }
    }
}

/// Where in the input an error or repair occurred.
///
/// New kinds of position may be added, so matches outside this crate need a
/// wildcard arm. Use [`Position::Unknown`] when no offset is meaningful,
/// never a placeholder such as `Buffer(0)`. The order is arbitrary but total,
/// so positions can be part of a set or map key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Hash)]
#[non_exhaustive]
pub enum Position {
    /// No byte position is meaningful or known.
    #[default]
    Unknown,
    /// Byte offset in the original file.
    File(usize),
    /// Byte offset in the decoded data of a stream object.
    Stream {
        /// The stream object.
        object: ObjectId,
        /// Offset in its decoded data.
        offset: usize,
    },
    /// Byte offset in a standalone buffer that is not part of a document.
    Buffer(usize),
}

impl Position {
    /// The stream object of a [`Position::Stream`].
    pub const fn object(self) -> Option<ObjectId> {
        match self {
            Self::Stream { object, .. } => Some(object),
            _ => None,
        }
    }

    /// Re-expresses an offset in a standalone buffer as a position in the
    /// input that buffer was taken from, given where the buffer starts.
    ///
    /// Only a [`Position::Buffer`] changes: its offset is added to `start`'s
    /// offset (saturating). Other positions, and any position when `start` is
    /// [`Position::Unknown`], are returned unchanged. This is how
    /// PDF-agnostic parsers (fonts, image codecs) report offsets that the
    /// PDF layer then places in the stream or file they came from.
    #[must_use]
    pub const fn within(self, start: Position) -> Self {
        let Self::Buffer(offset) = self else {
            return self;
        };
        match start {
            Self::Unknown => self,
            Self::File(base) => Self::File(base.saturating_add(offset)),
            Self::Stream {
                object,
                offset: base,
            } => Self::Stream {
                object,
                offset: base.saturating_add(offset),
            },
            Self::Buffer(base) => Self::Buffer(base.saturating_add(offset)),
        }
    }
}

impl fmt::Display for Position {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown => f.write_str("unknown position"),
            Self::File(offset) => write!(f, "file byte {offset}"),
            Self::Stream { object, offset } => {
                write!(f, "byte {offset} of stream {object}")
            }
            Self::Buffer(offset) => write!(f, "buffer byte {offset}"),
        }
    }
}

/// A Pagura error: what went wrong, where, and in which object.
///
/// Build errors with the constructors ([`Error::syntax`], [`Error::limit`],
/// …) and add context with the builders. Every builder keeps the innermost
/// context: a position or object recorded closer to the fault is more
/// precise than one an outer caller knows, so it is never overwritten.
/// Fields stay public for reading; the type is `#[non_exhaustive]` so other
/// crates cannot build it from a struct literal.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Error {
    /// The category callers branch on.
    pub kind: ErrorKind,
    /// A fixed human-readable description.
    pub message: &'static str,
    /// Where the problem was detected.
    pub position: Position,
    /// The indirect object being processed, when known.
    pub object: Option<ObjectId>,
    /// For a strict-mode rejection, the repair `Recovery::Repair` would have
    /// applied instead (ADR 0003).
    pub repair: Option<Repair>,
}

impl Error {
    /// Creates an error without position or object context. Prefer the
    /// constructor named after the kind, such as [`Error::syntax`].
    pub const fn new(kind: ErrorKind, message: &'static str) -> Self {
        Self {
            kind,
            message,
            position: Position::Unknown,
            object: None,
            repair: None,
        }
    }

    /// A syntax or structure violation.
    pub const fn syntax(message: &'static str) -> Self {
        Self::new(ErrorKind::Syntax, message)
    }

    /// An exceeded resource budget.
    pub const fn limit(message: &'static str) -> Self {
        Self::new(ErrorKind::Limit, message)
    }

    /// A valid input that needs the unimplemented `feature`.
    pub const fn unsupported(feature: Feature, message: &'static str) -> Self {
        Self::new(ErrorKind::Unsupported(feature), message)
    }

    /// A requested object that does not exist.
    pub const fn missing_object(message: &'static str) -> Self {
        Self::new(ErrorKind::MissingObject, message)
    }

    /// A cycle in a graph that must be acyclic.
    pub const fn cycle(message: &'static str) -> Self {
        Self::new(ErrorKind::Cycle, message)
    }

    /// A missing or incorrect password.
    pub const fn password(message: &'static str) -> Self {
        Self::new(ErrorKind::Password, message)
    }

    /// An invalid caller-supplied argument.
    pub const fn invalid_argument(message: &'static str) -> Self {
        Self::new(ErrorKind::InvalidArgument, message)
    }

    /// Sets the position unless one is already known.
    ///
    /// The object context stays separate: inside an object stream the
    /// position names the stream while the object is the compressed object
    /// being parsed. Where the stream itself is the object, add
    /// `.in_object_opt(position.object())`.
    #[must_use]
    pub const fn at(mut self, position: Position) -> Self {
        if matches!(self.position, Position::Unknown) {
            self.position = position;
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

    /// [`Error::in_object`] for an object that may not be known.
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
    /// An unknown position stays unknown.
    #[must_use]
    pub const fn within(mut self, start: Position) -> Self {
        self.position = self.position.within(start);
        let object = self.position.object();
        self.in_object_opt(object)
    }

    /// Records the repair that `Recovery::Repair` would have applied, unless
    /// one is already recorded (ADR 0003).
    #[must_use]
    pub const fn with_repair(mut self, repair: Repair) -> Self {
        if self.repair.is_none() {
            self.repair = Some(repair);
        }
        self
    }

    /// The missing capability, for `Unsupported` errors.
    pub const fn feature(&self) -> Option<Feature> {
        match self.kind {
            ErrorKind::Unsupported(feature) => Some(feature),
            _ => None,
        }
    }

    /// Whether this error reports an unimplemented capability.
    pub const fn is_unsupported(&self) -> bool {
        matches!(self.kind, ErrorKind::Unsupported(_))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind, self.message)?;
        if let Some(object) = self.object {
            write!(f, " (object {object})")?;
        }
        if !matches!(self.position, Position::Unknown) {
            write!(f, " at {}", self.position)?;
        }
        if let Some(repair) = self.repair {
            write!(f, "; recovery would repair it [{repair}]")?;
        }
        Ok(())
    }
}

impl std::error::Error for Error {}

/// Result type used across Pagura.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// The [`Error`] context builders, applied to the error of a [`Result`].
///
/// `parse(data).in_object(id)?` reads better than
/// `parse(data).map_err(|error| error.in_object(id))?`. Each method keeps the
/// innermost context exactly like the [`Error`] method of the same name.
pub trait ResultExt: Sized {
    /// [`Error::at`] on the error, if any.
    #[must_use]
    fn at(self, position: Position) -> Self;

    /// [`Error::in_object`] on the error, if any.
    #[must_use]
    fn in_object(self, id: ObjectId) -> Self;

    /// [`Error::in_object_opt`] on the error, if any.
    #[must_use]
    fn in_object_opt(self, id: Option<ObjectId>) -> Self;

    /// [`Error::within`] on the error, if any.
    #[must_use]
    fn within(self, start: Position) -> Self;
}

impl<T> ResultExt for Result<T> {
    fn at(self, position: Position) -> Self {
        self.map_err(|error| error.at(position))
    }

    fn in_object(self, id: ObjectId) -> Self {
        self.map_err(|error| error.in_object(id))
    }

    fn in_object_opt(self, id: Option<ObjectId>) -> Self {
        self.map_err(|error| error.in_object_opt(id))
    }

    fn within(self, start: Position) -> Self {
        self.map_err(|error| error.within(start))
    }
}

#[cfg(test)]
mod tests {
    use super::{Error, Feature, Position, ResultExt};
    use crate::{ObjectId, Repair};

    const DCT: Feature = Feature::new("filter.DCTDecode");

    #[test]
    fn innermost_context_wins_and_display_is_complete() {
        let error = Error::unsupported(DCT, "image codec is not implemented")
            .at(Position::File(10))
            .in_object(ObjectId::new(4, 0))
            .at(Position::File(99))
            .in_object(ObjectId::new(1, 0));
        assert_eq!(error.feature(), Some(DCT));
        assert_eq!(error.position, Position::File(10));
        assert_eq!(
            error.to_string(),
            "unsupported [filter.DCTDecode]: image codec is not implemented \
             (object 4 0) at file byte 10"
        );
    }

    #[test]
    fn stream_positions_leave_the_object_context_to_the_caller() {
        let stream = ObjectId::new(7, 0);
        let at = Position::Stream {
            object: stream,
            offset: 3,
        };
        assert_eq!(Error::syntax("bad").at(at).object, None);
        assert_eq!(at.object(), Some(stream));
        assert_eq!(Position::File(3).object(), None);
        // A compressed object keeps its own identity inside its stream.
        let compressed = Error::syntax("bad").at(at).in_object(ObjectId::new(2, 0));
        assert_eq!(
            (compressed.position, compressed.object),
            (at, Some(ObjectId::new(2, 0)))
        );
        let content = Error::syntax("bad").at(at).in_object_opt(at.object());
        assert_eq!(content.object, Some(stream));
        let placed = Error::syntax("bad").at(Position::File(1)).at(at);
        assert_eq!(placed.position, Position::File(1));
    }

    #[test]
    fn buffer_offsets_are_placed_within_their_source() {
        let stream = ObjectId::new(9, 0);
        let start = Position::Stream {
            object: stream,
            offset: 100,
        };
        let error = Error::syntax("bad").at(Position::Buffer(5)).within(start);
        assert_eq!(
            error.position,
            Position::Stream {
                object: stream,
                offset: 105
            }
        );
        assert_eq!(error.object, Some(stream));
        assert_eq!(
            Position::Buffer(5).within(Position::File(10)),
            Position::File(15)
        );
        assert_eq!(
            Position::Buffer(usize::MAX).within(Position::Buffer(1)),
            Position::Buffer(usize::MAX)
        );
        assert_eq!(
            Position::Buffer(5).within(Position::Unknown),
            Position::Buffer(5)
        );
        assert_eq!(
            Position::File(5).within(Position::File(10)),
            Position::File(5)
        );
        let unknown = Error::syntax("bad").within(start);
        assert_eq!(unknown.position, Position::Unknown);
        assert_eq!(unknown.object, None);
    }

    #[test]
    fn optional_objects_and_repairs_keep_the_innermost_value() {
        const FIRST: Repair = Repair::new("test.first");
        const SECOND: Repair = Repair::new("test.second");
        let error = Error::syntax("bad")
            .in_object_opt(None)
            .in_object_opt(Some(ObjectId::new(3, 0)))
            .in_object_opt(Some(ObjectId::new(4, 0)))
            .with_repair(FIRST)
            .with_repair(SECOND);
        assert_eq!(error.object, Some(ObjectId::new(3, 0)));
        assert_eq!(error.repair, Some(FIRST));
    }

    #[test]
    fn result_extension_applies_builders_only_to_errors() {
        let id = ObjectId::new(5, 0);
        let ok: crate::Result<u8> = Ok(1);
        assert_eq!(ok.in_object(id).at(Position::File(1)), Ok(1));
        let error: crate::Result<u8> = Err(Error::syntax("bad"));
        let error = error
            .at(Position::Buffer(2))
            .within(Position::File(10))
            .in_object_opt(Some(id))
            .unwrap_err();
        assert_eq!(error.position, Position::File(12));
        assert_eq!(error.object, Some(id));
    }
}
