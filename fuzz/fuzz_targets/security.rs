//! Password authentication and decryption. See `pagura_fuzz::security`.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| pagura_fuzz::security::run(data));
