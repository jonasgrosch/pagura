#![forbid(unsafe_code)]
#![warn(unreachable_pub)]
//! Deterministic serialization of new PDF documents (ISO 32000-2 clause 7.5).
//!
//! Output depends only on the inserted objects: object order is ascending
//! by number, dictionary entries keep their insertion order, and strings
//! are written in hexadecimal. Output intentionally targets PDF 1.7; it is not
//! a PDF 2.0 new-file writer (7.5.2). [`write_incremental`] appends a
//! revision to a caller-validated source without changing its original bytes.
//!
//! The [`Limits`](pagura_core::Limits) given to [`Builder::new`] bound the
//! written file as they bound reading it (ADR 0029): a file written under
//! some limits is read back under the same limits.

mod builder;
mod incremental;
mod object;
mod validation;
mod writer;

pub use incremental::{XrefFormat, write_incremental};
pub use validation::object as validate_object;

pub use builder::Builder;
pub use object::NewObject;
