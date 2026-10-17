//! Font programs, glyph outlines and advances. See `pagura_fuzz::font`.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| pagura_fuzz::font::run(data));
