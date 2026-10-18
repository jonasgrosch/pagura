#![forbid(unsafe_code)]
//! Pagura: an independent PDF engine in safe Rust.
//!
//! This facade re-exports the stable entry points of the workspace crates.
//! See `docs/STATUS.md` for what is implemented; unimplemented
//! capabilities are reported as [`ErrorKind::Unsupported`] errors carrying
//! a [`Feature`] tag, never as silently incomplete output.
//!
//! ```
//! use pagura::{Document, OpenOptions, RenderOptions};
//!
//! # fn main() -> pagura::Result<()> {
//! let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus/generated/classic/rectangle.pdf"))
//!     .expect("fixture exists");
//! let document = Document::open(bytes, &OpenOptions::default())?;
//! let page = document.page(0)?;
//! let pixmap = pagura::render_page(&document, page, &RenderOptions::default())?;
//! assert_eq!((pixmap.width(), pixmap.height()), (64, 64));
//! # Ok(())
//! # }
//! ```

pub use pagura_core::{
    Diagnostic, Dictionary, Error, ErrorKind, Feature, Limits, Name, ObjectId, Position, Recovery,
    Rect, Repair, Result, Value,
};
pub use pagura_document::{Document, IndirectObject, OpenOptions, Page, Rotation};
pub use pagura_raster::{Pixmap, Rgba8};
pub use pagura_render::{
    DisplayList, Provider, RenderContext, RenderOptions, Tile, compile_page_with, device_size,
    render_page_with, render_tile_with,
};
pub use pagura_write::{Builder, NewObject};

use std::sync::Arc;

/// Creates document-bound rendering resources using the feature-selected data.
/// Keep this context when compiling or rendering many pages.
pub fn render_context(document: Arc<Document>) -> RenderContext {
    #[cfg(feature = "bundled-data")]
    {
        let provider = Arc::new(pagura_data::Bundled::new(document.limits()));
        RenderContext::with_provider(document, provider)
    }
    #[cfg(not(feature = "bundled-data"))]
    RenderContext::new(document)
}

/// Compiles one page. For many pages, keep a [`render_context`]. The default
/// `bundled-data` feature provides standard-font substitutes and predefined CMaps.
///
/// # Errors
/// As [`pagura_render::compile_page`] and the feature-selected provider.
pub fn compile_page(document: &Document, page: &Page) -> Result<DisplayList> {
    #[cfg(feature = "bundled-data")]
    return compile_page_with(
        document,
        page,
        &pagura_data::Bundled::new(document.limits()),
    );
    #[cfg(not(feature = "bundled-data"))]
    pagura_render::compile_page(document, page)
}

/// Renders one page. For many pages, keep a [`render_context`].
///
/// # Errors
/// As [`compile_page`] and [`DisplayList::render`].
pub fn render_page(document: &Document, page: &Page, options: &RenderOptions) -> Result<Pixmap> {
    compile_page(document, page)?.render(options)
}

/// Renders one tile. For repeated work, keep a [`render_context`].
///
/// # Errors
/// As [`compile_page`] and [`DisplayList::render_tile`].
pub fn render_tile(
    document: &Document,
    page: &Page,
    options: &RenderOptions,
    tile: Tile,
) -> Result<Pixmap> {
    compile_page(document, page)?.render_tile(options, tile)
}

/// Licence notices for font and CMap data linked by this facade's features. Distributors must
/// reproduce these notices; the CLI exposes them through `pagura licenses`.
pub fn data_notices() -> impl Iterator<Item = (&'static str, &'static str)> {
    #[cfg(feature = "bundled-data")]
    return pagura_data::notices();
    #[cfg(not(feature = "bundled-data"))]
    std::iter::empty()
}
