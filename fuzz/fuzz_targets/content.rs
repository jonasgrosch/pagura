//! Content streams on a fixed page, compiled and rendered in tiles. See `pagura_fuzz::content`.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| pagura_fuzz::content::run(data));
