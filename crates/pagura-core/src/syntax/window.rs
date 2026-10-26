//! Extendable slice boundaries use the existing parser, without reclassifying arbitrary syntax errors.

use super::Parser;
use crate::{Diagnostic, Error, ErrorKind, Result};

/// Whether the end of a parser slice is also the true end of its input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum InputEnd {
    /// The source or decoded stream has no bytes beyond this slice.
    Complete,
    /// Bytes beyond this slice may complete a token or structure.
    Extendable,
}

/// One completed parser operation and its provisional diagnostics.
#[derive(Debug)]
#[non_exhaustive]
pub struct ParsedWindow<T> {
    /// The operation's completed value.
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
    /// No token or structure used an unavailable part of the input.
    Complete(ParsedWindow<T>),
    /// The parser reached an extendable boundary; no diagnostics are published.
    Incomplete {
        /// Smallest longer slice that could make progress; larger bounded windows are allowed.
        minimum_length: usize,
    },
}

impl<'a> Parser<'a> {
    /// Runs one operation with an explicit input boundary, consuming the parser.
    ///
    /// The callback uses the same parser methods as complete-input callers. It
    /// must retain side effects until this method returns a completed value.
    /// Boundary detection occurs at token/structure ends, not on each byte.
    /// An incomplete attempt discards its values and diagnostics; resource and
    /// non-syntax errors retain their terminal meaning.
    ///
    /// # Errors
    /// Returns terminal parser/callback errors. Only a structurally observed
    /// extendable boundary can defer a syntax result; malformed complete input
    /// keeps its existing error and position.
    pub fn read_window<T>(
        mut self,
        end: InputEnd,
        read: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<WindowRead<T>> {
        self.window_end = end;
        self.incomplete_window = false;
        let result = read(&mut self);
        let may_extend = result
            .as_ref()
            .err()
            .is_none_or(|error| error.kind == ErrorKind::Syntax);
        if self.incomplete_window && may_extend {
            let minimum_length = self
                .data
                .len()
                .checked_add(1)
                .ok_or(Error::limit("parser window length overflow"))?;
            return Ok(WindowRead::Incomplete { minimum_length });
        }
        result.map(|value| {
            WindowRead::Complete(ParsedWindow {
                value,
                offset: self.offset(),
                diagnostics: self.take_diagnostics(),
            })
        })
    }

    pub(super) fn window_boundary(&mut self) {
        self.incomplete_window |= self.window_end == InputEnd::Extendable;
    }
}
