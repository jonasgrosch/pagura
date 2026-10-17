//! Stream filters and predictors within their output cap. See `pagura_fuzz::filters`.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| pagura_fuzz::filters::run(data));
