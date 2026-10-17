//! Rendering a page either returns a complete surface of the advertised size
//! or an error; never a panic or a surface of the wrong size.
#![no_main]

use libfuzzer_sys::fuzz_target;
use pagura::{Document, Limits, OpenOptions, RenderOptions};

fuzz_target!(|data: &[u8]| {
    let options = OpenOptions {
        limits: Limits {
            operations: 100_000,
            pixels: 1 << 16,
            ..Limits::default()
        },
        ..OpenOptions::default()
    };
    let Ok(document) = Document::open(data.to_vec(), &options) else {
        return;
    };
    let Ok(page) = document.page(0) else {
        return;
    };
    let render = RenderOptions::from_dpi(18.0);
    if let (Ok(size), Ok(pixmap)) = (pagura::device_size(page, &render), pagura::render_page(&document, page, &render)) {
        assert_eq!((pixmap.width(), pixmap.height()), size, "surface size differs from device_size");
    }
});
