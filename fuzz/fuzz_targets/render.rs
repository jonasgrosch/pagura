//! Rendering the first page at the advertised size. See `pagura_fuzz::render`.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| pagura_fuzz::render::run(data));
