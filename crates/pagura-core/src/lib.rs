#![forbid(unsafe_code)]
#![warn(unreachable_pub)]
//! PDF primitives shared by every Pagura crate: the object model, bounded
//! syntax parsing (ISO 32000-2 clauses 7.2–7.3), structured errors, recovery
//! diagnostics and resource limits.
//!
//! It also holds the primitives every parser shares, so no crate needs its
//! own copy:
//! - [`Meter`], the work and memory budget charged against [`Limits`];
//! - [`bytes::Cursor`] and [`bytes::BitCursor`] for binary formats;
//! - [`text_string`], the PDF text-string and PDFDocEncoding codec;
//! - [`Diagnostics::repair_once`], [`Error::within`] and [`ResultExt`] for
//!   reporting with context;
//! - [`convert`], lossless integer widening;
//! - [`Store`], the one budgeted cache of reusable rendering values.
//!
//! This crate has no knowledge of files, revisions or rendering. It is the
//! vocabulary the other crates speak.

pub mod bytes;
pub mod convert;
mod diagnostic;
mod dictionary;
mod error;
pub mod hash;
mod limits;
mod meter;
mod name;
mod rect;
pub mod repairs;
mod retained;
mod rotation;
mod store;
pub mod syntax;
pub mod text_string;
mod value;

pub use diagnostic::{Diagnostic, DiagnosticKind, Diagnostics, Recovery, Repair};
pub use dictionary::{Dictionary, DictionaryIter};
pub use error::{Error, ErrorKind, Feature, Position, Result, ResultExt};
pub use limits::{Limits, SignatureLimits};
pub use meter::Meter;
pub use name::Name;
pub use rect::Rect;
pub use retained::INDEX_ENTRY_OVERHEAD;
pub use rotation::Rotation;
pub use store::{Built, Store, StoreKey};
pub use value::{ObjectId, Value};
