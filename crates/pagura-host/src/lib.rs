#![forbid(unsafe_code)]
//! Bounded, capability-free PDF sessions over stdio (ADR 0005).
//!
//! Only the command's standard input/output are used. PDF interpretation is
//! delegated to the facade and cannot execute host commands or perform I/O.

mod cache;
mod control;
mod editing;
pub mod protocol;
pub mod reading;
mod session;
mod wire;

pub use control::Cancellation;
pub use session::{Session, SessionEnd};
