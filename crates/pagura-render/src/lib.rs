#![forbid(unsafe_code)]
//! Page rendering: content-stream interpretation into pixels (ADR 0004).
//!
//! [`compile_page`] builds an immutable vector [`DisplayList`] with
//! source-space colours, winding fills, strokes, clip stacks and form
//! expansion. The list renders into device tiles through `pagura-raster`.
//! [`render_page`] and [`render_tile`] compile and render in one call.

mod blend;
mod cie;
mod color;
mod compositor;
mod context;
mod direct_image;
mod display;
mod functions;
mod icc;
mod interpreter;
mod operators;
mod optional;
mod overprint;
mod pattern;
mod shading;
mod transparency;

pub use context::RenderContext;
pub use display::DisplayList;
pub use interpreter::{
    compile_filled_page, compile_page, compile_page_with, compile_page_with_text, text_page,
};
pub use overprint::OverprintPreview;

use pagura_core::{Error, Rect, Result};
use pagura_document::{Document, Page, Rotation};
use pagura_raster::Rgba8;

pub use pagura_raster::Pixmap;
pub use pagura_text::{Provider, SearchOptions, TextMatch, TextPage};

/// Stable tags for rendering capabilities not implemented yet.
pub mod features;
/// In-memory form appearance programs and resource dictionaries (12.7.4.3).
pub mod form_appearance;

/// Options for rendering a page.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderOptions {
    /// Device pixels per default user-space unit (1/72 inch); 1.0 is 72 dpi.
    pub scale: f64,
    /// Colour the page is composited onto.
    pub background: Rgba8,
    /// Whether to simulate overprinting (8.6.7; 11.7.4).
    pub overprint: OverprintPreview,
    /// How images are decoded and sampled.
    pub images: ImageReconstruction,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            scale: 1.0,
            background: Rgba8::WHITE,
            overprint: OverprintPreview::Auto,
            images: ImageReconstruction::Exact,
        }
    }
}

/// How images are decoded and sampled (ADR 0004).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum ImageReconstruction {
    /// Each pixel is the exact-area mean of the decoders' samples, rounded
    /// once, as the image sampling reference defines it.
    #[default]
    Exact,
    /// A page whose only mark is one opaque DeviceGray or DeviceRGB image,
    /// upright or turned by a quarter, is decoded, converted and filtered in
    /// one pass on the original sample grid, and painted straight into the
    /// output. Every pixel is within one level of [`Self::Exact`]. Other
    /// pages render as [`Self::Exact`], after a second compilation when the
    /// page's first mark was an image.
    ///
    /// This applies where a page is compiled and rendered in one call
    /// ([`render_page`], [`render_tile`] and their [`RenderContext`]
    /// versions). A list from [`compile_page`] always holds exact samples.
    Fused,
}

impl RenderOptions {
    /// Options for a resolution in dots per inch.
    pub fn from_dpi(dpi: f64) -> Self {
        Self {
            scale: dpi / 72.0,
            ..Self::default()
        }
    }
}

/// A rectangle of device pixels within a rendered page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Tile {
    /// Left edge in device pixels.
    pub x: u32,
    /// Top edge in device pixels.
    pub y: u32,
    /// Width in device pixels.
    pub width: u32,
    /// Height in device pixels.
    pub height: u32,
}

/// The size in device pixels of `page` rendered with `options`, after
/// crop box, user unit, rotation and scale.
///
/// # Errors
///
/// Returns `InvalidArgument` for a non-positive or non-finite scale and a
/// limit error if the size does not fit `u32`.
pub fn device_size(page: &Page, options: &RenderOptions) -> Result<(u32, u32)> {
    device_extent(page.crop_box(), page.rotation(), page.user_unit(), options)
}

/// Device size for a crop box, rotation and user unit (clause 8.3.2).
fn device_extent(
    crop: Rect,
    rotation: Rotation,
    user_unit: f64,
    options: &RenderOptions,
) -> Result<(u32, u32)> {
    if !(options.scale.is_finite() && options.scale > 0.0) {
        return Err(Error::invalid_argument("scale must be positive and finite"));
    }
    let factor = options.scale * user_unit;
    let (width, height) = match rotation {
        Rotation::None | Rotation::Clockwise180 => (crop.width(), crop.height()),
        Rotation::Clockwise90 | Rotation::Clockwise270 => (crop.height(), crop.width()),
    };
    Ok((pixels(width * factor)?, pixels(height * factor)?))
}

/// Rounds a device extent up to whole pixels, ignoring floating-point
/// noise just above an integer.
fn pixels(extent: f64) -> Result<u32> {
    if !extent.is_finite() || extent <= 0.0 {
        return Err(Error::limit("device extent is not finite and positive"));
    }
    let rounded = (extent - 1e-9).ceil().max(1.0);
    if rounded > f64::from(u32::MAX) {
        return Err(Error::limit("device size exceeds u32"));
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "range checked above; value is a positive integer"
    )]
    Ok(rounded as u32)
}

/// Renders a whole page.
///
/// # Errors
///
/// Returns `Unsupported` for content this renderer cannot draw, a limit
/// error for budget violations, and syntax errors for malformed content.
/// No partial page is ever returned.
pub fn render_page(document: &Document, page: &Page, options: &RenderOptions) -> Result<Pixmap> {
    let fonts = std::sync::Mutex::new(interpreter::text::cache::FontCache::new(document.limits()));
    direct_image::render(
        |layout| interpreter::compile_cached_with(document, page, None, &fonts, layout),
        options,
        None,
    )
}

/// Renders a whole page using explicitly supplied external font resources.
///
/// # Errors
/// As [`render_page`], including errors from the provider.
pub fn render_page_with(
    document: &Document,
    page: &Page,
    options: &RenderOptions,
    provider: &dyn Provider,
) -> Result<Pixmap> {
    let fonts = std::sync::Mutex::new(interpreter::text::cache::FontCache::new(document.limits()));
    direct_image::render(
        |layout| interpreter::compile_cached_with(document, page, Some(provider), &fonts, layout),
        options,
        None,
    )
}

/// Renders one tile of a page. Tiles of one page with the same options
/// assemble byte for byte into the [`render_page`] result. To render many
/// tiles of a page, compile it once with [`compile_page`] and call
/// [`DisplayList::render_tile`].
///
/// # Errors
///
/// As [`render_page`], plus `InvalidArgument` for a tile outside the page.
pub fn render_tile(
    document: &Document,
    page: &Page,
    options: &RenderOptions,
    tile: Tile,
) -> Result<Pixmap> {
    let fonts = std::sync::Mutex::new(interpreter::text::cache::FontCache::new(document.limits()));
    direct_image::render(
        |layout| interpreter::compile_cached_with(document, page, None, &fonts, layout),
        options,
        Some(tile),
    )
}

/// Renders a tile using explicitly supplied external font resources.
///
/// # Errors
/// As [`render_tile`], including errors from the provider.
pub fn render_tile_with(
    document: &Document,
    page: &Page,
    options: &RenderOptions,
    tile: Tile,
    provider: &dyn Provider,
) -> Result<Pixmap> {
    let fonts = std::sync::Mutex::new(interpreter::text::cache::FontCache::new(document.limits()));
    direct_image::render(
        |layout| interpreter::compile_cached_with(document, page, Some(provider), &fonts, layout),
        options,
        Some(tile),
    )
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod recovery_tests;
