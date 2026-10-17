#![forbid(unsafe_code)]
//! Bounded, capability-free PDF sessions over stdio (ADR 0005).
//!
//! Only the command's standard input/output are used. PDF interpretation is
//! delegated to the facade and cannot execute host commands or perform I/O.

mod cache;
pub mod protocol;
mod session;

pub use session::{Session, SessionEnd};
