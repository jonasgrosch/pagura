#![forbid(unsafe_code)]
//! PDF primitives shared by every Pagura crate: the object model, bounded
//! syntax parsing (ISO 32000-2 clauses 7.2–7.3), structured errors, recovery
//! diagnostics and resource limits.
//!
//! This crate has no knowledge of files, revisions or rendering. It is the
//! vocabulary the other crates speak.

pub mod bytes;
mod diagnostic;
mod dictionary;
mod error;
mod limits;
mod meter;
mod name;
mod rect;
mod retained;
pub mod syntax;
mod value;

pub use diagnostic::{Diagnostic, DiagnosticKind, Diagnostics, Recovery, Repair};
pub use dictionary::{Dictionary, DictionaryIter};
pub use error::{Error, ErrorKind, Feature, Position, Result, ResultExt};
pub use limits::Limits;
pub use meter::Meter;
pub use name::Name;
pub use rect::Rect;
pub use retained::INDEX_ENTRY_OVERHEAD;
pub use value::{ObjectId, Value};
