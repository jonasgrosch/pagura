//! Object parsing in strict and repair mode. See `pagura_fuzz::syntax`.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| pagura_fuzz::syntax::run(data));
