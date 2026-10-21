//! `syntax`: every object parse either succeeds or returns a structured
//! error, in both recovery modes (ISO 32000-2 clauses 7.2–7.3).

use pagura_core::syntax::{Origin, Parser};
use pagura_core::{Limits, Recovery};

/// Parses objects until the first error.
///
/// # Panics
///
/// When the input violates a checked property: that is a finding.
pub fn run(data: &[u8]) {
    for recovery in [Recovery::Strict, Recovery::Repair] {
        let mut parser =
            Parser::new(data, Origin::Buffer, Limits::default()).with_recovery(recovery);
        while super::checked(parser.parse()).is_some() {}
        super::assert_diagnostics(&parser.take_diagnostics(), recovery);
    }
}
