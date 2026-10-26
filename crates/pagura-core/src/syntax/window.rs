//! Typed window failures share the same parser as complete inputs.

use super::Parser;
use crate::{Diagnostic, Error, Result};
use std::{convert::Infallible, fmt, marker::PhantomData};

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
pub struct WindowInput<C = Infallible> {
    end: InputEnd,
    pending: bool,
    callback: PhantomData<fn() -> C>,
}
impl<C> sealed::Sealed for WindowInput<C> {}
impl<C: fmt::Debug> ParseMode for WindowInput<C> {
    type Fault = ParseFault<C>;
    fn end(&self, error: Error) -> ParseFault<C> {
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
    fn map_error(fault: ParseFault<C>, map: impl FnOnce(Error) -> Error) -> ParseFault<C> {
        match fault {
            ParseFault::Incomplete => ParseFault::Incomplete,
            ParseFault::Invalid(error) => ParseFault::Invalid(map(error)),
            ParseFault::Deferred(control) => ParseFault::Deferred(control),
        }
    }
}

/// A window callback must distinguish unavailable input from terminal failure.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ParseFault<C = Infallible> {
    /// A structural read needs more bytes in this window.
    Incomplete,
    /// A terminal lexical, semantic, resource or callback error.
    Invalid(Error),
    /// A callback cannot complete until its own typed dependency is satisfied.
    /// This neither extends the current window nor publishes provisional data.
    Deferred(C),
}
impl<C> From<Error> for ParseFault<C> {
    fn from(error: Error) -> Self {
        Self::Invalid(error)
    }
}
impl<C> fmt::Display for ParseFault<C> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Incomplete => formatter.write_str("parser window is incomplete"),
            Self::Invalid(error) => fmt::Display::fmt(error, formatter),
            Self::Deferred(_) => formatter.write_str("parser callback is deferred"),
        }
    }
}
impl<C: fmt::Debug> std::error::Error for ParseFault<C> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Incomplete | Self::Deferred(_) => None,
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

/// A completed operation, incomplete input window or deferred callback.
#[derive(Debug)]
#[non_exhaustive]
pub enum WindowRead<T, C = Infallible> {
    /// No token or structure remains dependent on extending the input.
    Complete(ParsedWindow<T>),
    /// The parser needs more bytes; no provisional diagnostics are published.
    Incomplete {
        /// Smallest longer slice that could make progress.
        minimum_length: usize,
    },
    /// A callback dependency stopped this attempt; no value or diagnostics
    /// are published. The caller owns its dependency policy and retry.
    Deferred(C),
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
        self.read_window_with_control(end, read)
    }

    /// Runs the same window parser with caller-owned dependency control.
    ///
    /// A callback may return [`ParseFault::Deferred`] when it must read another
    /// semantic dependency. This is distinct from an incomplete current slice
    /// and from terminal syntax, including at true EOF. Deferred attempts
    /// discard their values and diagnostics without extending the window.
    ///
    /// # Errors
    /// As [`Self::read_window`]. Only a typed deferred result can pause a
    /// callback; terminal errors are never reclassified after a boundary read.
    pub fn read_window_with_control<T, C: fmt::Debug>(
        self,
        end: InputEnd,
        read: impl FnOnce(&mut Parser<'a, WindowInput<C>>) -> std::result::Result<T, ParseFault<C>>,
    ) -> Result<WindowRead<T, C>> {
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
                callback: PhantomData,
            },
        };
        match read(&mut parser) {
            Err(ParseFault::Invalid(error)) => Err(error),
            Err(ParseFault::Deferred(control)) => Ok(WindowRead::Deferred(control)),
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
