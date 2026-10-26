#![forbid(unsafe_code)]
#![warn(unreachable_pub)]
//! Bounded, capability-free PDF sessions over stdio (ADR 0005).
//!
//! Only the command's standard input/output are used. PDF interpretation is
//! delegated to the facade and cannot execute host commands or perform I/O.
//! Session policy has its own budgets, [`SessionLimits`], next to the
//! engine's [`pagura::Limits`] (ADR 0032).

mod cache;
mod config;
mod control;
mod editing;
pub mod progressive;
pub mod protocol;
pub mod reading;
mod session;
mod wire;

pub use config::{SessionConfig, SessionLimits};
pub use control::Cancellation;
pub use progressive::ProgressiveLimits;
pub use session::{Session, SessionEnd};
