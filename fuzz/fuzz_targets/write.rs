//! Writer round trips: parse(write(v)) == v. See `pagura_fuzz::write`.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| pagura_fuzz::write::run(data));
