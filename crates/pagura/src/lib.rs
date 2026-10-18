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
pub use pagura_document::{IndirectObject, OpenOptions, Page, Rotation};
pub use pagura_raster::{Pixmap, Rgba8};
pub use pagura_render::{
    DisplayList, Provider, RenderContext, RenderOptions, Tile, compile_page_with, device_size,
    render_page_with, render_tile_with,
};
pub use pagura_write::{Builder, NewObject};

mod document;
pub use document::Document;

/// Compiles a page with the document's shared rendering resources. The default
/// `bundled-data` feature provides standard-font substitutes and predefined CMaps.
///
/// # Errors
/// As [`RenderContext::compile_page`].
pub fn compile_page(document: &Document, page: &Page) -> Result<DisplayList> {
    document.render_context().compile_page(page)
}

/// Renders a whole page using this document's shared fonts and glyph outlines.
///
/// # Errors
/// As [`RenderContext::render_page`].
pub fn render_page(document: &Document, page: &Page, options: &RenderOptions) -> Result<Pixmap> {
    document.render_context().render_page(page, options)
}

/// Renders a tile using this document's shared fonts and glyph outlines.
///
/// # Errors
/// As [`RenderContext::render_tile`].
pub fn render_tile(
    document: &Document,
    page: &Page,
    options: &RenderOptions,
    tile: Tile,
) -> Result<Pixmap> {
    document.render_context().render_tile(page, options, tile)
}

/// Licence notices for data linked by this facade's features. Distributors must
/// reproduce these notices; the CLI exposes them through `pagura licenses`.
pub fn data_notices() -> impl Iterator<Item = (&'static str, &'static str)> {
    #[cfg(feature = "bundled-data")]
    return pagura_data::notices();
    #[cfg(not(feature = "bundled-data"))]
    std::iter::empty()
}
