//! `render`: recording a page and rendering it, exactly and with every fast
//! parameter, returns a complete surface of the advertised size or a
//! structured error; never a panic or a surface of the wrong size (ADR 0004,
//! ADR 0034).

use pagura::{Document, FastParams, OpenOptions, RenderOptions};
use std::sync::Arc;

/// Opens the input, records its first page once and renders it at 18 dpi,
/// exactly and competitively.
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
    let context = pagura::render_context(Arc::new(document));
    let document = context.document();
    let Some(page) = super::checked(document.page(0)) else {
        return;
    };
    if let Some(scene) = super::checked(context.scene(page)) {
        for fast in [FastParams::EXACT, FastParams::COMPETITIVE] {
            let render = RenderOptions {
                fast,
                ..RenderOptions::from_dpi(18.0)
            };
            let size = super::checked(pagura::device_size(page, &render));
            let pixmap = super::checked(context.render_scene(&scene, &render, None));
            if let (Some(size), Some(pixmap)) = (size, pixmap) {
                assert_eq!(
                    (pixmap.width(), pixmap.height()),
                    size,
                    "{fast:?} surface size differs from device_size"
                );
            }
        }
    }
    super::assert_diagnostics(&document.diagnostics(), options.recovery);
}
