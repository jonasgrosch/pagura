#![forbid(unsafe_code)]
#![warn(unreachable_pub)]
//! Page rendering and text extraction (ADR 0034): the thin layer that ties
//! the interpreter, the renderer and the store together.
//!
//! A page is interpreted once into a device-independent [`Scene`]
//! (`pagura-interpret`), which a [`RenderContext`] keeps in its
//! [`Store`]. Rendering a region of it at a resolution
//! maps the page to device pixels ([`RenderRequest::page`](pagura_paint::RenderRequest::page)),
//! prepares the scene for that region (`pagura_paint::prepare` validates
//! every mark that meets the region and reserves every budget in source
//! order), reserves output storage and
//! runs the band executor, which cannot fail. Output can be collected into
//! a [`Pixmap`] or consumed one band at a time through
//! [`RenderContext::render_bands`]. Repairs that preparation makes are
//! reported to the document. Engine validation and allocation finish before
//! the first consumer call; a consumer can return its own error, such as an
//! I/O failure, after output starts.
//!
//! Every pixel is within one level of its exact result (ADR 0004). Text
//! extraction runs the same interpreter into its text device, which decodes
//! no image, parses no shading and converts no colour.
//!
//! The free functions record and render one page with a store of their own;
//! keep a [`RenderContext`] to reuse scenes, fonts, glyphs and images across
//! renders, tiles and pages.

mod bounds;
mod context;
mod options;
mod renderer;

pub use context::RenderContext;
pub use options::{OverprintPreview, RenderOptions, Tile};
pub use pagura_extract::{PositionedText, TextPage};
pub use pagura_interpret::Scene;
pub use pagura_paint::{BandLease, BandOutput, Completion, OutputLimits, RenderSession};
pub use pagura_raster::Pixmap;
pub use pagura_text::Provider;

use pagura_core::{ObjectId, Rect, Result, Store};
use pagura_document::source::ReadResult;
use pagura_document::{Document, Page};
use pagura_interpret::PageSpace;
use pagura_paint::{RenderRequest, Scratch};
use renderer::Renderer;

/// The size in device pixels of `page` rendered with `options`: its crop
/// box under `UserUnit`, `Rotate` and the scale, rounded up (8.3.2).
///
/// # Errors
///
/// Returns `InvalidArgument` for a non-positive or non-finite scale and a
/// `Limit` error when the size does not fit `u32`.
pub fn device_size(page: &Page, options: &RenderOptions) -> Result<(u32, u32)> {
    let space = PageSpace {
        crop: page.crop_box(),
        rotation: page.rotation(),
        user_unit: page.user_unit(),
    };
    let request = RenderRequest::page(&space, options.scale)?;
    Ok((request.page.width(), request.page.height()))
}

/// Records `page` into a scene with stored appearances, using `provider` for
/// fonts the document does not embed (ADR 0009).
///
/// # Errors
///
/// As [`RenderContext::scene`].
pub fn record_page(
    document: &Document,
    page: &Page,
    provider: Option<&dyn Provider>,
) -> ReadResult<Scene> {
    with_renderer(document, provider, |renderer| renderer.record(page))
}

/// Records and renders a whole page.
///
/// # Errors
///
/// As [`RenderContext::render_page`]. No partial page is ever returned.
pub fn render_page(
    document: &Document,
    page: &Page,
    options: &RenderOptions,
    provider: Option<&dyn Provider>,
) -> ReadResult<Pixmap> {
    with_renderer(document, provider, |renderer| {
        let scene = renderer.record(page)?;
        renderer
            .render(&scene, (options, None), &mut Scratch::default(), |_| false)
            .map(|(pixmap, _)| pixmap)
    })
}

/// Records a page and renders one tile of it. To render many tiles, keep a
/// [`RenderContext`], which records the page once.
///
/// # Errors
///
/// As [`RenderContext::render_tile`].
pub fn render_tile(
    document: &Document,
    page: &Page,
    options: &RenderOptions,
    tile: Tile,
    provider: Option<&dyn Provider>,
) -> ReadResult<Pixmap> {
    with_renderer(document, provider, |renderer| {
        let scene = renderer.record(page)?;
        renderer
            .render(
                &scene,
                (options, Some(tile)),
                &mut Scratch::default(),
                |_| false,
            )
            .map(|(pixmap, _)| pixmap)
    })
}

/// The positioned text of `page` through the text device.
///
/// # Errors
///
/// As [`RenderContext::text_page`].
pub fn text_page(
    document: &Document,
    page: &Page,
    provider: Option<&dyn Provider>,
) -> ReadResult<TextPage> {
    with_renderer(document, provider, |renderer| renderer.text_page(page))
}

/// The positioned text of one stored annotation appearance, fitted into
/// `rect`, without interpreting the page.
///
/// # Errors
///
/// As [`RenderContext::appearance_text`].
pub fn appearance_text(
    document: &Document,
    page: &Page,
    appearance: ObjectId,
    rect: Rect,
    provider: Option<&dyn Provider>,
) -> ReadResult<PositionedText> {
    with_renderer(document, provider, |renderer| {
        renderer.appearance_text(page, appearance, rect)
    })
}

/// Runs `work` with a store of its own, dropped afterwards.
fn with_renderer<T>(
    document: &Document,
    provider: Option<&dyn Provider>,
    work: impl FnOnce(Renderer<'_>) -> ReadResult<T>,
) -> ReadResult<T> {
    let store = Store::new(RenderContext::DEFAULT_STORE_BYTES);
    work(Renderer {
        document,
        provider,
        store: &store,
    })
}

#[cfg(test)]
mod testing;
#[cfg(test)]
mod tests;
