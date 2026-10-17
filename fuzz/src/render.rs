//! `render`: rendering a page either returns a complete surface of the
//! advertised size or a structured error; never a panic or a surface of the
//! wrong size (ADR 0004).

use pagura::{Document, OpenOptions, RenderOptions};

/// Opens the input and renders its first page at 18 dpi.
///
/// # Panics
///
/// When the input violates a checked property: that is a finding.
pub fn run(data: &[u8]) {
    let options = OpenOptions {
        limits: super::limits(),
        ..OpenOptions::default()
    };
    let Some(document) = super::checked(Document::open(data.to_vec(), &options)) else {
        return;
    };
    let Some(page) = super::checked(document.page(0)) else {
        return;
    };
    let render = RenderOptions::from_dpi(18.0);
    let size = super::checked(pagura::device_size(page, &render));
    let pixmap = super::checked(pagura::render_page(&document, page, &render));
    if let (Some(size), Some(pixmap)) = (size, pixmap) {
        assert_eq!(
            (pixmap.width(), pixmap.height()),
            size,
            "surface size differs from device_size"
        );
    }
    super::assert_diagnostics(&document.diagnostics(), options.recovery);
}
