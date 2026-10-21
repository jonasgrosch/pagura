//! Form loading, in-memory filling and appearance preparation. See `pagura_fuzz::forms`.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| pagura_fuzz::forms::run(data));
