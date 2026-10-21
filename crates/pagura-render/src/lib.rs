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
mod fragments;
mod functions;
mod hash;
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

/// The accuracy contract of a rendering (ADR 0004).
///
/// The exact path is the reference and the default. The competitive path is
/// approximate: it is measured against the competitive accuracy gate of ADR
/// 0004, and can always be switched back to the exact path.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Accuracy {
    /// Within one level of the stage-specific references in ADR 0004.
    /// Composition is evaluated on already quantized coverage bytes; this
    /// does not establish a one-level bound against unquantized coverages or
    /// a filtered continuous scene.
    #[default]
    Exact,
    /// One faster approximation per primitive, chosen against the competitive
    /// accuracy gate:
    /// - Solid-colour glyphs in admitted 16-bit or f32 storage use masks built
    ///   from row fragments reused across horizontal origins. Origins snap to
    ///   1/128 pixel
    ///   horizontally, and vertically to 1/128 pixel below 108 dpi, 1/16
    ///   pixel from 108 dpi and 1/64 pixel from 216 dpi; curves become
    ///   0.15-pixel chords below 108 dpi and area-balanced 1/4-pixel chords
    ///   from 108 dpi.
    /// - Other fills and strokes accumulate signed areas in cells after
    ///   flattening curves to 1/64-pixel chords. Compound paths without a
    ///   retained stroke boundary keep geometric integration of their union.
    /// - A clipping path that contains a mark is dropped for it, and a
    ///   quadrilateral mark within quadrilateral clips is integrated with them
    ///   geometrically. A rectangular clip multiplies into a mark as a column
    ///   factor times a row factor; any other clipping path as its own cell
    ///   coverage, integrated once per band.
    /// - Normal composition preserves the planner's admitted 16-bit or f32
    ///   storage in bounded chunks, on this policy's coverage bytes. Shading
    ///   patterns can also use admitted f32 storage. Retained groups and other
    ///   exclusions keep the general f64 storage.
    /// - A page whose only mark is one opaque DeviceGray or DeviceRGB image,
    ///   upright or turned by a quarter, keeps a baseline JPEG in its
    ///   component planes and one-bit samples packed, and is converted and
    ///   filtered row by row straight into the output, within one level of
    ///   the exact result. This applies where a page is compiled and
    ///   rendered in one call ([`render_page`], [`render_tile`] and their
    ///   [`RenderContext`] versions).
    /// - Minified opaque DeviceGray or DeviceRGB JPEG images drawn from page
    ///   content are reconstructed with 1, 2, 3, 4 or 6 cells per block
    ///   axis, at least two and a half per device pixel. Remaining images
    ///   are sampled exactly.
    /// - Embedded TrueType programs are decoded only as far as drawing reads
    ///   them, so damage in later tables is neither examined nor reported.
    ///
    /// A limit error from competitive compositing triggers one retry of the
    /// same display list with exact compositing. The retry retains the
    /// compilation's image samples and font-validation policy; it may still
    /// fail. Compilation and direct-image attempts can add work before this
    /// retry. This policy does not meet the one-level contract.
    Competitive,
}

/// Options for rendering a page.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderOptions {
    /// Device pixels per default user-space unit (1/72 inch); 1.0 is 72 dpi.
    pub scale: f64,
    /// Colour the page is composited onto.
    pub background: Rgba8,
    /// Whether to simulate overprinting (8.6.7; 11.7.4).
    pub overprint: OverprintPreview,
    /// The accuracy contract pixels are rendered to (ADR 0004).
    pub accuracy: Accuracy,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            scale: 1.0,
            background: Rgba8::WHITE,
            overprint: OverprintPreview::Auto,
            accuracy: Accuracy::Exact,
        }
    }
}

impl RenderOptions {
    /// Options for a resolution in dots per inch.
    pub fn from_dpi(dpi: f64) -> Self {
        Self {
            scale: dpi / 72.0,
            ..Self::default()
        }
    }

    /// Device pixels per default user-space unit of `page` when compiling
    /// for [`Accuracy::Competitive`], at which minified images may be
    /// decoded near their drawn size; None on the exact path.
    pub(crate) fn drawn_scale(&self, page: &Page) -> Option<f64> {
        let scale = self.scale * page.user_unit();
        (self.accuracy == Accuracy::Competitive && scale.is_finite() && scale > 0.0)
            .then_some(scale)
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
    render_compiled(document, page, options, None, None)
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
    render_compiled(document, page, options, Some(provider), None)
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
    render_compiled(document, page, options, None, Some(tile))
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
    render_compiled(document, page, options, Some(provider), Some(tile))
}

/// Compiles `page` for one rendering and renders it whole or as `tile`, with
/// a font cache of its own.
fn render_compiled(
    document: &Document,
    page: &Page,
    options: &RenderOptions,
    provider: Option<&dyn Provider>,
    tile: Option<Tile>,
) -> Result<Pixmap> {
    let fonts = std::sync::Mutex::new(interpreter::text::cache::FontCache::new(document.limits()));
    direct_image::render(
        |layout| {
            interpreter::compile_cached_with(
                document,
                page,
                provider,
                &fonts,
                (layout, None, options.drawn_scale(page)),
            )
        },
        options,
        tile,
        (None, compositor::Lent::default()),
    )
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod recovery_tests;
