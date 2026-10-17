//! Structured errors shared by every Pagura crate.

use crate::{ObjectId, Repair};
use std::fmt;

/// A stable, machine-readable tag for an unimplemented PDF capability.
///
/// Tags are `area.item`, for example `filter.DCTDecode`, `operator.Tj`,
/// `colorspace.ICCBased` or `security.PubSec`. Areas are lowercase; items use
/// the PDF spelling. Scoreboards and API callers group failures by tag, so a
/// tag must not change once published. Each crate defines its own constants.
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// Creates an error without position or object context.
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

    /// Sets the position. An already known position is kept, because the
    /// innermost position is the most precise one.
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

#[cfg(test)]
mod tests {
    use super::{Error, Feature, Position};
    use crate::ObjectId;

    #[test]
    fn innermost_context_wins_and_display_is_complete() {
        const DCT: Feature = Feature::new("filter.DCTDecode");
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
}
