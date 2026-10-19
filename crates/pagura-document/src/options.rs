//! How a document is opened: budgets, recovery and the password.

use pagura_core::{Limits, Recovery};
use std::fmt;

/// How to open a document.
///
/// A plain record built with functional record update, as [`Limits`] is:
/// `OpenOptions { recovery: Recovery::Strict, ..OpenOptions::default() }`.
/// The document keeps the limits and recovery mode, never the password.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct OpenOptions {
    /// Resource budgets for every operation on the document.
    pub limits: Limits,
    /// Whether malformed input may be repaired (reported) or is an error.
    pub recovery: Recovery,
    /// Password for encrypted documents; empty tries the empty password.
    pub password: Vec<u8>,
}

impl fmt::Debug for OpenOptions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenOptions")
            .field("limits", &self.limits)
            .field("recovery", &self.recovery)
            .field("password", &"[redacted]")
            .finish()
    }
}
