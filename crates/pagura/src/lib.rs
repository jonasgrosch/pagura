#![forbid(unsafe_code)]
//! Pagura: an independent PDF engine in safe Rust.
//!
//! Positioned text extraction and search go through [`text_page`], or
//! [`RenderContext::text_page`] to share fonts across pages. Extraction has its
//! own output and leaves the rendering display list unchanged.
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
    Diagnostic, DiagnosticKind, Dictionary, Error, ErrorKind, Feature, Limits, Name, ObjectId,
    Position, Recovery, Rect, Repair, Result, SignatureLimits, Value,
};
/// Interactive form source views and in-memory filling (12.7). Actions are inert data.
pub use pagura_document::forms;
/// Outlines, destinations, links, actions and page labels (12.3, 12.4, 12.5.6.5, 12.6).
/// Actions are data: Pagura never follows a URI, opens a file or runs a script.
pub use pagura_document::navigation;
pub mod signatures;
pub use pagura_document::{Document, IndirectObject, OpenOptions, Page, Rotation};
pub use pagura_raster::{Pixmap, Rgba8};
/// Form appearance programs and resources kept as data (12.7.4.3).
pub use pagura_render::form_appearance;
pub use pagura_render::{
    DisplayList, OverprintPreview, Provider, RenderContext, RenderOptions, SearchOptions,
    TextMatch, TextPage, Tile, compile_page_with, compile_page_with_text, device_size,
    render_page_with, render_tile_with,
};
pub use pagura_write::{Builder, NewObject};

use std::sync::Arc;

/// Creates document-bound rendering resources using the feature-selected data.
/// Keep this context when compiling or rendering many pages. The opt-in
/// `cjk-fonts` feature adds horizontal collection-specific substitutes.
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
/// `bundled-data` feature provides standard-font substitutes and predefined CMaps;
/// `cjk-fonts` adds horizontal CID substitutes and is off by default.
/// This uses stored appearances and ignores NeedAppearances. Use
/// [`compile_filled_page`] with an empty filler to honor NeedAppearances.
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

/// Compiles a page using in-memory filled values (12.7.4.3).
/// Uses the feature-selected font provider, as [`compile_page`]. Keep a
/// [`render_context`] to share resources when re-rendering after edits.
///
/// # Errors
/// As [`pagura_render::compile_filled_page`].
pub fn compile_filled_page(filler: &forms::FormFiller<'_>, page: &Page) -> Result<DisplayList> {
    #[cfg(feature = "bundled-data")]
    return pagura_render::compile_filled_page(
        filler,
        page,
        Some(&pagura_data::Bundled::new(filler.document().limits())),
    );
    #[cfg(not(feature = "bundled-data"))]
    pagura_render::compile_filled_page(filler, page, None)
}

/// Extracts one page's positioned text, with the feature-selected font data.
/// For many pages, keep a [`render_context`] and use
/// [`RenderContext::text_page`].
///
/// # Errors
/// As [`compile_page`] and [`compile_page_with_text`].
pub fn text_page(document: &Document, page: &Page) -> Result<TextPage> {
    #[cfg(feature = "bundled-data")]
    return pagura_render::text_page(
        document,
        page,
        Some(&pagura_data::Bundled::new(document.limits())),
    );
    #[cfg(not(feature = "bundled-data"))]
    pagura_render::text_page(document, page, None)
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

/// Prepares normal widget appearances for an in-memory form session (12.7.4.3).
/// Uses the feature-selected font provider, as [`compile_page`]. The returned
/// programs and document-relative resources remain data; nothing is saved.
///
/// # Errors
/// As [`form_appearance::prepare`], including explicit unsupported capabilities.
pub fn prepare_form(
    filler: &forms::FormFiller<'_>,
) -> Result<Vec<form_appearance::WidgetAppearance>> {
    #[cfg(feature = "bundled-data")]
    return form_appearance::prepare(
        filler,
        Some(&pagura_data::Bundled::new(filler.document().limits())),
    );
    #[cfg(not(feature = "bundled-data"))]
    form_appearance::prepare(filler, None)
}

/// Licence notices for font and CMap data, including opt-in CJK outlines,
/// linked by this facade's features. Distributors must
/// reproduce these notices; the CLI exposes them through `pagura licenses`.
pub fn data_notices() -> impl Iterator<Item = (&'static str, &'static str)> {
    #[cfg(feature = "bundled-data")]
    return pagura_data::notices();
    #[cfg(not(feature = "bundled-data"))]
    std::iter::empty()
}

/// Inspects signature fields, byte ranges and revision coverage (12.7.5.5, 12.8).
/// Checks CMS integrity for supported encodings. Trust, modification analysis,
/// validation time and revocation remain unchecked.
///
/// ```
/// # fn main() -> pagura::Result<()> {
/// # let document = pagura::Document::open(
/// #     include_bytes!("../../../corpus/generated/signatures/structure.pdf").as_slice(),
/// #     &pagura::OpenOptions::default(),
/// # )?;
/// let views = pagura::signatures(&document)?;
/// assert_eq!(views.fields.len(), 1);
/// let signature = views.fields[0].signature.as_ref().expect("populated field");
/// assert_eq!(signature.results.signer_trust, pagura::signatures::SignerTrust::NotCheckedYet);
/// # Ok(())
/// # }
/// ```
///
/// # Errors
/// As [`Document::signatures`].
pub fn signatures(document: &Document) -> Result<signatures::Signatures<'_>> {
    signatures::verify(document)
}
