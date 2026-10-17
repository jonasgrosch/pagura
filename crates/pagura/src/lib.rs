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
    DisplayList, RenderOptions, Tile, compile_page, device_size, render_page, render_tile,
};
pub use pagura_write::{Builder, NewObject};
