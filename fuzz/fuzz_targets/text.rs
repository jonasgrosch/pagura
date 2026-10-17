//! PDF font dictionaries, CMaps and string decoding. See `pagura_fuzz::text`.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| pagura_fuzz::text::run(data));
