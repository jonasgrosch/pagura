//! Trees, destinations, outlines, links, actions and page labels. See `pagura_fuzz::navigation`.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| pagura_fuzz::navigation::run(data));
