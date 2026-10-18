//! Entries of the constant inventory tables.
//!
//! Each table is linked only with its cargo feature and is empty without it,
//! so a lookup then finds nothing and no data bytes are linked.
use pagura_core::{Diagnostics, Limits, Recovery, Result};
use pagura_font::{FontFormat, FontProgram};

/// A pinned data file and the name it is looked up by.
#[derive(Debug)]
pub(crate) struct File {
    pub(crate) key: &'static [u8],
    pub(crate) data: &'static [u8],
}

/// A pinned font program, the name it is looked up by and its format.
#[derive(Debug)]
pub(crate) struct Program {
    pub(crate) key: &'static [u8],
    pub(crate) data: &'static [u8],
    pub(crate) format: FontFormat,
}

impl Program {
    /// Parses the program strictly: a pinned file that needs repair is a
    /// packaging error. Unlike `FontProgram::parse`, this keeps an exhausted
    /// budget a `Limit` error.
    pub(crate) fn parse(&self, limits: &Limits) -> Result<FontProgram> {
        FontProgram::parse_with_diagnostics(
            self.data,
            self.format,
            limits,
            &mut Diagnostics::new(Recovery::Strict),
        )
    }
}
