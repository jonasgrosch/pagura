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
//!   resources, [`tags`] the stable tags of rendering, and [`forms`],
//!   [`form_appearance`], [`navigation`] and [`signatures`](mod@signatures)
//!   their domains.
//!
//! A page is recorded once into a device-independent [`Scene`] and rendered
//! from it by region at any resolution (ADR 0034), every pixel within one
//! level of its exact result (ADR 0004). Text extraction runs the same
//! interpreter without painting.
//!
//! The free functions ([`record_page`], [`text_page`], [`render_page`],
//! [`render_tile`], [`appearance_text`], [`prepare_form`]) use the
//! feature-selected [`provider::default_provider`]. To record or render many
//! pages, or many tiles of one page, keep a [`render_context`], whose store
//! keeps scenes, fonts, glyphs and images between renders; to use other font
//! data, build one with [`render_context_with_provider`]. A page with
//! in-memory form values renders through [`RenderContext::filled_scene`] and
//! [`RenderContext::render_scene`].
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
/// Reported repair of contradicting JBIG2 page capability declarations.
pub use pagura_image::jbig2::repairs::PAGE_CAPABILITIES as JBIG2_PAGE_CAPABILITIES;
/// Reported repair of an unused JBIG2 Huffman aggregation-table selector.
pub use pagura_image::jbig2::repairs::UNUSED_AGGREGATION_TABLE as JBIG2_UNUSED_AGGREGATION_TABLE;
pub mod editing;
pub mod signatures;
/// Form-field and annotation appearance generation, kept as data (12.5.5,
/// 12.7.4.3). Its functions take an explicit provider; [`prepare_form`] uses
/// the feature-selected one.
pub use pagura_appearance as form_appearance;
/// Reported missing separator between a source unsigned integer and `endobj`.
pub use pagura_document::repairs::INTEGER_SEPARATOR as OBJECT_INTEGER_SEPARATOR;
pub use pagura_document::{
    Document, ImageCodec, ImageStream, IndirectObject, ObjectSource, OpenOptions, Page, Rotation,
};
/// Reported viewing repair for unboxed JPEG 2000 with explicit PDF colour.
pub use pagura_image::repairs::JPX_CODESTREAM as JPX_CODESTREAM_REPAIR;
pub use pagura_raster::{Pixmap, Point, Rgba8};
pub use pagura_render::{
    Completion, OverprintPreview, RenderContext, RenderOptions, Scene, Tile, device_size,
};
pub use pagura_write::{Builder, NewObject};
pub use text::{SearchOptions, TextMatch, TextPage};

/// Stable [`Feature`] tags of rendering (ADR 0017), by the crate that owns
/// their areas: compare them with [`Error::feature`]. Each module lists its
/// tags in `ALL`. Text and font tags stay with their errors' own crates;
/// [`form_appearance::information`] holds the appearance information tags.
pub mod tags {
    /// `annotation.` and `appearance.`: appearance generation.
    pub use pagura_appearance::features as appearance;
    /// `colorspace.` and `function.`: colour spaces and functions.
    pub use pagura_color::features as color;
    /// `image.`, `jpeg.` and `ccitt.`: image samples and decoders.
    pub use pagura_image::features as image;
    /// `graphics.`, `extgstate.`, `optionalcontent.`, `pattern.`,
    /// `shading.`, `transparency.` and `xobject.`: content interpretation
    /// and painting.
    pub use pagura_interpret::features as content;
}

use provider::{Provider, default_provider};
use std::sync::Arc;

/// Creates document-bound rendering resources with the feature-selected
/// [`provider::default_provider`] and the default store capacity
/// ([`RenderContext::DEFAULT_STORE_BYTES`]). Keep this context when recording
/// or rendering many pages. The opt-in `cjk-fonts` feature adds horizontal
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

/// Records one page into a scene with stored appearances, with the
/// feature-selected font data. For many pages, keep a [`render_context`],
/// whose [`RenderContext::scene`] keeps the scene for later renders. The
/// default `bundled-data` feature provides standard-font substitutes and
/// predefined CMaps; `cjk-fonts` adds horizontal CID substitutes and is off
/// by default. `NeedAppearances` is honoured only by
/// [`RenderContext::filled_scene`].
///
/// # Errors
/// As [`RenderContext::scene`].
pub fn record_page(document: &Document, page: &Page) -> Result<Scene> {
    let provider = default_provider(document.limits());
    pagura_render::record_page(document, page, provider.as_deref())
}

/// Extracts actual positioned text from a stored widget appearance.
/// Uses the feature-selected font provider, without interpreting the page.
///
/// # Errors
/// As [`RenderContext::appearance_text`].
pub fn appearance_text(
    document: &Document,
    page: &Page,
    appearance: ObjectId,
    rect: Rect,
) -> Result<text::PositionedText> {
    let provider = default_provider(document.limits());
    pagura_render::appearance_text(document, page, appearance, rect, provider.as_deref())
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
/// As [`RenderContext::render_page`].
pub fn render_page(document: &Document, page: &Page, options: &RenderOptions) -> Result<Pixmap> {
    let provider = default_provider(document.limits());
    pagura_render::render_page(document, page, options, provider.as_deref())
}

/// Renders one tile. For repeated work, keep a [`render_context`], which
/// records the page once for all its tiles.
///
/// # Errors
/// As [`RenderContext::render_tile`].
pub fn render_tile(
    document: &Document,
    page: &Page,
    options: &RenderOptions,
    tile: Tile,
) -> Result<Pixmap> {
    let provider = default_provider(document.limits());
    pagura_render::render_tile(document, page, options, tile, provider.as_deref())
}

/// Prepares normal widget appearances for an in-memory form session (12.7.4.3).
/// Uses the feature-selected font provider, as [`render_page`]. The returned
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
