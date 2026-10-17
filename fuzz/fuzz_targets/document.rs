//! Opening, enumerating pages and compiling content never panics, in both
//! recovery modes, with budgets small enough to keep each run fast.
#![no_main]

use libfuzzer_sys::fuzz_target;
use pagura::{Document, Limits, OpenOptions, Recovery};

fuzz_target!(|data: &[u8]| {
    for recovery in [Recovery::Strict, Recovery::Repair] {
        let options = OpenOptions {
            limits: Limits {
                operations: 100_000,
                pixels: 1 << 16,
                ..Limits::default()
            },
            recovery,
            ..OpenOptions::default()
        };
        let Ok(document) = Document::open(data.to_vec(), &options) else {
            continue;
        };
        let _ = document.diagnostics();
        if let Ok(pages) = document.pages() {
            for page in pages.iter().take(4) {
                let _ = pagura::compile_page(&document, page);
            }
        }
    }
});
