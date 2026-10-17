//! Framed session requests through pagura-host. See `pagura_fuzz::host`.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| pagura_fuzz::host::run(data));
