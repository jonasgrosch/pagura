//! Every object parse either succeeds or returns an error; never a panic.
#![no_main]

use libfuzzer_sys::fuzz_target;
use pagura_core::syntax::{Origin, Parser};
use pagura_core::{Limits, Recovery};

fuzz_target!(|data: &[u8]| {
    for recovery in [Recovery::Strict, Recovery::Repair] {
        let mut parser = Parser::new(data, Origin::Buffer, Limits::default()).with_recovery(recovery);
        while parser.parse().is_ok() {}
    }
});
