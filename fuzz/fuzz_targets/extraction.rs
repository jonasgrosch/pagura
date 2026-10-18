//! Text extraction and search on a fixed page. See `pagura_fuzz::extraction`.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| pagura_fuzz::extraction::run(data));
