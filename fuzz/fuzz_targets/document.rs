//! Opening, lazy objects, streams, pages and content compilation. See `pagura_fuzz::document`.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| pagura_fuzz::document::run(data));
