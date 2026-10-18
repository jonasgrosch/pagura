#![forbid(unsafe_code)]
#![warn(unreachable_pub)]
//! Deterministic serialization of new PDF documents (ISO 32000-2 clause 7.5).
//!
//! Output depends only on the inserted objects: object order is ascending
//! by number, dictionary entries keep their insertion order, and strings
//! are written in hexadecimal. Output intentionally targets PDF 1.7; it is not
//! a PDF 2.0 writer (7.5.2). Incremental saving is not implemented yet.

mod builder;
mod object;
mod writer;

pub use builder::Builder;
pub use object::NewObject;
