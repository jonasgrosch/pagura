//! Why a command failed at run time (exit status 1).

use std::{fmt, io};

/// A run-time failure: the engine's structured error, an I/O error, or a
/// request that does not fit the document.
#[derive(Debug)]
pub(crate) enum Failure {
    /// A typed source wait or a terminal engine failure.
    Engine(pagura::ReadFault),
    /// Reading the input or writing an output failed.
    Io(io::Error),
    /// The request does not fit the document, such as a page outside it.
    Request(String),
}

impl Failure {
    /// The `kind` of the failure in JSON reports: the engine's error kind,
    /// `io` or `request`.
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Engine(error) => error
                .as_error()
                .map_or("unavailable", |error| error.kind.name()),
            Self::Io(_) => "io",
            Self::Request(_) => "request",
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Engine(error) => error.fmt(f),
            Self::Io(error) => error.fmt(f),
            Self::Request(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for Failure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Engine(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::Request(_) => None,
        }
    }
}

impl From<pagura::Error> for Failure {
    fn from(error: pagura::Error) -> Self {
        Self::Engine(error.into())
    }
}

impl From<pagura::ReadFault> for Failure {
    fn from(fault: pagura::ReadFault) -> Self {
        Self::Engine(fault)
    }
}

impl From<io::Error> for Failure {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
