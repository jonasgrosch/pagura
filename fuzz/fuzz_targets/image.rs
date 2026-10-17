//! Image codecs, image XObjects and inline images. See `pagura_fuzz::image`.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| pagura_fuzz::image::run(data));
