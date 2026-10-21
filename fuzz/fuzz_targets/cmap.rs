//! Predefined CMap programs and code decoding. See `pagura_fuzz::cmap`.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| pagura_fuzz::cmap::run(data));
