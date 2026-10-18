#![forbid(unsafe_code)]
#![warn(unreachable_pub)]
//! Pagura: an independent PDF engine in safe Rust.
//!
//! This facade is the public API of the workspace crates. Its imports are
//! enough to use every item it exposes:
//!
//! - the root holds documents, pages, the object model, rendering and
//!   writing, the free functions, and the types their signatures name;
//! - [`text`] holds the extracted-text model, [`provider`] the font and CMap
//!   resources, and [`forms`], [`form_appearance`], [`navigation`] and
//!   [`signatures`](mod@signatures) their domains.
//!
//! The free functions ([`compile_page`], [`compile_page_with_text`],
//! [`text_page`], [`render_page`], [`render_tile`], [`prepare_form`]) use the
//! feature-selected [`provider::default_provider`]. To compile or render many
//! pages, keep a [`render_context`]; to use other font data, build one with
//! [`render_context_with_provider`]. Positioned text extraction has its own
//! output and leaves the rendering display list unchanged.
//!
//! [`Document`] and [`RenderContext`] are `Send + Sync`: share one through an
//! `Arc` across threads. See `docs/STATUS.md` for what is implemented;
//! unimplemented capabilities are reported as [`ErrorKind::Unsupported`]
//! errors carrying a [`Feature`] tag, never as silently incomplete output.
//!
//! ```
//! use pagura::{Builder, Dictionary, Document, Name, NewObject, ObjectId, OpenOptions, Value};
//!
//! # fn main() -> pagura::Result<()> {
//! // Write a one-page document that fills a square, then read it back.
//! let id = |number| ObjectId::new(number, 0);
//! let dictionary = |entries: &[(&[u8], Value)]| -> Value {
//!     let entries = entries.iter().map(|(key, value)| (Name::new(key), value.clone()));
//!     Value::Dictionary(entries.collect())
//! };
//! let mut builder = Builder::default();
//! let catalog = dictionary(&[(b"Type", Value::name(b"Catalog")), (b"Pages", Value::Reference(id(2)))]);
//! builder.insert(id(1), NewObject::Value(catalog))?;
//! let kids = Value::Array(vec![Value::Reference(id(3))]);
//! let pages = dictionary(&[(b"Type", Value::name(b"Pages")), (b"Kids", kids), (b"Count", Value::Integer(1))]);
//! builder.insert(id(2), NewObject::Value(pages))?;
//! let page = dictionary(&[
//!     (b"Type", Value::name(b"Page")),
//!     (b"Parent", Value::Reference(id(2))),
//!     (b"MediaBox", Value::Array([0, 0, 64, 64].map(Value::Integer).to_vec())),
//!     (b"Contents", Value::Reference(id(4))),
//! ]);
//! builder.insert(id(3), NewObject::Value(page))?;
//! let content = b"0 0 1 rg 16 16 32 32 re f".to_vec();
//! builder.insert(id(4), NewObject::Stream { dictionary: Dictionary::new(), data: content })?;
//!
//! let document = Document::open(builder.finish(id(1))?, &OpenOptions::default())?;
//! let options = pagura::RenderOptions::default();
//! let pixmap = pagura::render_page(&document, document.page(0)?, &options)?;
//! assert_eq!((pixmap.width(), pixmap.height()), (64, 64));
//! # Ok(())
//! # }
//! ```

pub mod provider;

/// Positioned text, its reading-order layout, structure and search
/// (ADR 0014), as returned by [`text_page`] and
/// [`RenderContext::text_page`].
pub mod text {
    pub use pagura_extract::{
        Character, LayoutOptions, LogicalText, MarkedContent, MarkedSpan, PositionedGlyph,
        PositionedText, Quad, SearchOptions, StructureElement, StructureNamespace, TextFont,
        TextLine, TextMatch, TextObject, TextPage, TextWord,
    };
    pub use pagura_text::{FontKind, WritingMode};
}

/// Lossless integer widening (`u32` to `usize`, `usize` to `u64`) on the
/// 32- and 64-bit targets Pagura supports, for callers of the facade alone.
pub use pagura_core::convert;
pub use pagura_core::{
    Diagnostic, DiagnosticKind, Dictionary, Error, ErrorKind, Feature, Limits, Name, ObjectId,
    Position, Recovery, Rect, Repair, Result, SignatureLimits, Value,
};
/// Interactive form source views and in-memory filling (12.7). Actions are inert data.
pub use pagura_document::forms;
/// Outlines, destinations, links, actions and page labels (12.3, 12.4, 12.5.6.5, 12.6).
/// Actions are data: Pagura never follows a URI, opens a file or runs a script.
pub use pagura_document::navigation;
pub mod editing;
pub mod signatures;
pub use pagura_document::{
    Document, ImageCodec, ImageStream, IndirectObject, ObjectSource, OpenOptions, Page, Rotation,
};
pub use pagura_raster::{Pixmap, Point, Rgba8};
/// Form appearance programs and resources kept as data (12.7.4.3). Its
/// functions take an explicit provider; [`prepare_form`] uses the
/// feature-selected one.
pub use pagura_render::form_appearance;
pub use pagura_render::{
    Accuracy, DisplayList, OverprintPreview, RenderContext, RenderOptions, Tile, device_size,
};
pub use pagura_write::{Builder, NewObject};
pub use text::{SearchOptions, TextMatch, TextPage};

use provider::{Provider, default_provider};
use std::sync::Arc;

/// Creates document-bound rendering resources with the feature-selected
/// [`provider::default_provider`]. Keep this context when compiling or
/// rendering many pages. The opt-in `cjk-fonts` feature adds horizontal
/// collection-specific substitutes.
///
/// ```
/// # fn example(document: pagura::Document) -> pagura::Result<()> {
/// let context = pagura::render_context(std::sync::Arc::new(document));
/// let options = pagura::RenderOptions::from_dpi(144.0);
/// for page in context.document().pages()? {
///     let pixmap = context.render_page(page, &options)?;
///     println!("{} x {}", pixmap.width(), pixmap.height());
/// }
/// # Ok(())
/// # }
/// ```
pub fn render_context(document: Arc<Document>) -> RenderContext {
    let provider = default_provider(document.limits());
    render_context_with_provider(document, provider)
}

/// Creates document-bound rendering resources with `provider`, or with only
/// the resources embedded in the PDF for `None`. Share one provider across
/// contexts to parse each resource once.
pub fn render_context_with_provider(
    document: Arc<Document>,
    provider: Option<Arc<dyn Provider>>,
) -> RenderContext {
    match provider {
        Some(provider) => RenderContext::with_provider(document, provider),
        None => RenderContext::new(document),
    }
}

/// Compiles one page. For many pages, keep a [`render_context`]. The default
/// `bundled-data` feature provides standard-font substitutes and predefined CMaps;
/// `cjk-fonts` adds horizontal CID substitutes and is off by default.
/// This uses stored appearances and ignores NeedAppearances. Use
/// [`compile_filled_page`] with an empty filler to honor NeedAppearances.
///
/// # Errors
/// As [`RenderContext::compile_page`].
pub fn compile_page(document: &Document, page: &Page) -> Result<DisplayList> {
    match default_provider(document.limits()) {
        Some(provider) => pagura_render::compile_page_with(document, page, provider.as_ref()),
        None => pagura_render::compile_page(document, page),
    }
}

/// Compiles a page using in-memory filled values (12.7.4.3).
/// Uses the feature-selected font provider, as [`compile_page`]. Keep a
/// [`render_context`] to share resources when re-rendering after edits.
///
/// # Errors
/// As [`pagura_render::compile_filled_page`].
pub fn compile_filled_page(filler: &forms::FormFiller<'_>, page: &Page) -> Result<DisplayList> {
    let provider = default_provider(filler.document().limits());
    pagura_render::compile_filled_page(filler, page, provider.as_deref())
}

/// Compiles one page and extracts its positioned text in the same pass, with
/// the feature-selected font data, as [`compile_page`] and [`text_page`] do.
/// For many pages, keep a [`render_context`].
///
/// # Errors
/// As [`RenderContext::compile_page_with_text`].
pub fn compile_page_with_text(document: &Document, page: &Page) -> Result<(DisplayList, TextPage)> {
    let provider = default_provider(document.limits());
    pagura_render::compile_page_with_text(document, page, provider.as_deref())
}

/// Extracts actual positioned text from a stored widget appearance.
/// Uses the feature-selected font provider, without interpreting the page.
///
/// # Errors
/// As [`pagura_render::appearance_text`].
pub fn appearance_text(
    document: &Document,
    page: &Page,
    appearance: ObjectId,
    rect: Rect,
) -> Result<pagura_render::PositionedText> {
    #[cfg(feature = "bundled-data")]
    return pagura_render::appearance_text(
        document,
        page,
        appearance,
        rect,
        Some(&pagura_data::Bundled::new(document.limits())),
    );
    #[cfg(not(feature = "bundled-data"))]
    pagura_render::appearance_text(document, page, appearance, rect, None)
}

/// Extracts one page's positioned text, with the feature-selected font data.
/// For many pages, keep a [`render_context`] and use
/// [`RenderContext::text_page`].
///
/// ```
/// # fn example(document: &pagura::Document) -> pagura::Result<()> {
/// let text = pagura::text_page(document, document.page(0)?)?;
/// println!("{}", text.text());
/// for found in text.find("invoice", pagura::SearchOptions::default())? {
///     println!("{:?} at {:?}", found.range, found.quads);
/// }
/// # Ok(())
/// # }
/// ```
///
/// # Errors
/// As [`RenderContext::text_page`].
pub fn text_page(document: &Document, page: &Page) -> Result<TextPage> {
    let provider = default_provider(document.limits());
    pagura_render::text_page(document, page, provider.as_deref())
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
    let provider = default_provider(filler.document().limits());
    form_appearance::prepare(filler, provider.as_deref())
}

/// Licence notices of the data this build links, as `(subject, text)`:
/// the Unicode data behind password preparation (always) and the font and
/// CMap data of the `bundled-data` and `cjk-fonts` features. Distributors
/// must reproduce every notice; the CLI prints them with `pagura licenses`.
///
/// ```
/// assert!(pagura::notices().any(|(subject, _)| subject.starts_with("Unicode")));
/// ```
pub fn notices() -> impl Iterator<Item = (&'static str, &'static str)> {
    let unicode = std::iter::once(("Unicode 3.2.0", pagura_crypto::unicode_notice()));
    #[cfg(feature = "bundled-data")]
    return unicode.chain(pagura_data::notices());
    #[cfg(not(feature = "bundled-data"))]
    unicode
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
