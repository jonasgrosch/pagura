//! Typed window failures share the same parser as complete inputs.

use super::Parser;
use crate::{Diagnostic, Error, Result};
use std::fmt;

mod sealed {
    pub trait Sealed {}
}

/// Failure policy of the shared parser; implementations are sealed.
#[doc(hidden)]
pub trait ParseMode: sealed::Sealed + fmt::Debug {
    /// The mode's terminal or incomplete failure.
    type Fault: From<Error>;
    /// Reports a structurally required byte beyond the current slice.
    fn end(&self, error: Error) -> Self::Fault;
    /// Records a successful lexical token whose boundary is not yet known.
    fn observe_end(&mut self);
    /// Whether a successful operation inspected an extendable boundary.
    fn pending(&self) -> bool;
    /// Whether this slice can be extended.
    fn extendable(&self) -> bool;
    /// Adds context to a terminal error without changing input availability.
    fn map_error(fault: Self::Fault, map: impl FnOnce(Error) -> Error) -> Self::Fault;
}

/// Complete-input parser mode, preserving the existing terminal error API.
#[derive(Debug)]
#[doc(hidden)]
pub struct CompleteInput;
impl sealed::Sealed for CompleteInput {}
impl ParseMode for CompleteInput {
    type Fault = Error;
    fn end(&self, error: Error) -> Error {
        error
    }
    fn observe_end(&mut self) {}
    fn pending(&self) -> bool {
        false
    }
    fn extendable(&self) -> bool {
        false
    }
    fn map_error(error: Error, map: impl FnOnce(Error) -> Error) -> Error {
        map(error)
    }
}

/// Whether a window ends at the true end of its source or decoded stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum InputEnd {
    /// No bytes follow this slice.
    Complete,
    /// More bytes may complete a token or structure.
    Extendable,
}

/// Window parser mode; constructed by [`Parser::read_window`].
#[derive(Debug)]
#[doc(hidden)]
pub struct WindowInput {
    end: InputEnd,
    pending: bool,
}
impl sealed::Sealed for WindowInput {}
impl ParseMode for WindowInput {
    type Fault = ParseFault;
    fn end(&self, error: Error) -> ParseFault {
        if self.extendable() {
            ParseFault::Incomplete
        } else {
            ParseFault::Invalid(error)
        }
    }
    fn observe_end(&mut self) {
        self.pending |= self.extendable();
    }
    fn pending(&self) -> bool {
        self.pending
    }
    fn extendable(&self) -> bool {
        self.end == InputEnd::Extendable
    }
    fn map_error(fault: ParseFault, map: impl FnOnce(Error) -> Error) -> ParseFault {
        match fault {
            ParseFault::Incomplete => ParseFault::Incomplete,
            ParseFault::Invalid(error) => ParseFault::Invalid(map(error)),
        }
    }
}

/// A window callback must distinguish unavailable input from terminal failure.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ParseFault {
    /// A structural read needs more bytes in this window.
    Incomplete,
    /// A terminal lexical, semantic, resource or callback error.
    Invalid(Error),
}
impl From<Error> for ParseFault {
    fn from(error: Error) -> Self {
        Self::Invalid(error)
    }
}
impl fmt::Display for ParseFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Incomplete => formatter.write_str("parser window is incomplete"),
            Self::Invalid(error) => fmt::Display::fmt(error, formatter),
        }
    }
}
impl std::error::Error for ParseFault {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Incomplete => None,
            Self::Invalid(error) => Some(error),
        }
    }
}

/// One completed parser operation and its diagnostics.
#[derive(Debug)]
#[non_exhaustive]
pub struct ParsedWindow<T> {
    /// The completed value.
    pub value: T,
    /// First unconsumed byte offset within the supplied slice.
    pub offset: usize,
    /// Diagnostics published only after this operation completed.
    pub diagnostics: Vec<Diagnostic>,
}

/// A completed operation or a structurally incomplete input window.
#[derive(Debug)]
#[non_exhaustive]
pub enum WindowRead<T> {
    /// No token or structure remains dependent on extending the input.
    Complete(ParsedWindow<T>),
    /// The parser needs more bytes; no provisional diagnostics are published.
    Incomplete {
        /// Smallest longer slice that could make progress.
        minimum_length: usize,
    },
}

impl<'a> Parser<'a> {
    /// Runs one operation with an explicit boundary, consuming this parser.
    ///
    /// The callback uses the same parser implementation with typed window
    /// failures. Its `Error` values convert only to [`ParseFault::Invalid`];
    /// observing a boundary never hides an independent terminal error. Keep
    /// side effects provisional until a completed value is returned.
    ///
    /// # Errors
    /// Returns terminal parser/callback errors without reclassification. An
    /// incomplete attempt discards its values and diagnostics. Returning an
    /// explicit incomplete failure for a declared complete input is invalid.
    pub fn read_window<T>(
        self,
        end: InputEnd,
        read: impl FnOnce(&mut Parser<'a, WindowInput>) -> std::result::Result<T, ParseFault>,
    ) -> Result<WindowRead<T>> {
        let mut parser = Parser {
            data: self.data,
            position: self.position,
            origin: self.origin,
            origin_offset: self.origin_offset,
            limits: self.limits,
            nodes: self.nodes,
            diagnostics: self.diagnostics,
            mode: WindowInput {
                end,
                pending: false,
            },
        };
        match read(&mut parser) {
            Err(ParseFault::Invalid(error)) => Err(error),
            Ok(value) if !parser.mode.pending => Ok(WindowRead::Complete(ParsedWindow {
                value,
                offset: parser.offset(),
                diagnostics: parser.take_diagnostics(),
            })),
            Ok(_) | Err(ParseFault::Incomplete) if end == InputEnd::Extendable => {
                let minimum_length = parser
                    .data
                    .len()
                    .checked_add(1)
                    .ok_or(Error::limit("parser window length overflow"))?;
                Ok(WindowRead::Incomplete { minimum_length })
            }
            Ok(_) | Err(ParseFault::Incomplete) => Err(Error::invalid_argument(
                "incomplete callback result for complete input",
            )),
        }
    }
}

impl<M: ParseMode> Parser<'_, M> {
    pub(super) fn window_boundary(&mut self) {
        self.mode.observe_end();
    }
    pub(super) fn end_error(&self, message: &'static str) -> M::Fault {
        self.mode.end(Error::syntax(message).at(self.position()))
    }
}
