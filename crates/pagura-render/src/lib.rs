#![forbid(unsafe_code)]
#![warn(unreachable_pub)]
//! Page rendering: content-stream interpretation into pixels (ADR 0004).
//!
//! [`compile_page`] builds an immutable vector [`DisplayList`] with
//! source-space colours, winding fills, strokes, clip stacks and form
//! expansion. The list renders into device tiles through `pagura-raster`
//! with [`DisplayList::render`] and [`DisplayList::render_tile`]. A
//! [`RenderContext`] shares fonts and document configuration across pages.

mod blend;
mod border;
mod cie;
mod color;
mod compositor;
mod context;
mod device;
mod display;
mod functions;
mod icc;
mod interpreter;
mod numbers;
mod operators;
mod optional;
mod overprint;
mod pattern;
mod shading;
mod transparency;

pub use context::RenderContext;
pub use display::DisplayList;
pub use interpreter::{
    compile_filled_page, compile_page, compile_page_with_provider, compile_page_with_text,
    text_page,
};
pub use overprint::OverprintPreview;

use pagura_core::{Error, Rect, Result};
use pagura_document::{Page, Rotation};
use pagura_raster::Rgba8;

pub use pagura_raster::Pixmap;
pub use pagura_text::{Provider, TextPage};

pub mod features;
/// In-memory form appearance programs and resource dictionaries (12.7.4.3).
pub mod form_appearance;
pub mod repairs;

/// Options for rendering a page. Start from [`RenderOptions::default`] or
/// [`RenderOptions::from_dpi`]; fields may be added.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct RenderOptions {
    /// Device pixels per default user-space unit (1/72 inch); 1.0 is 72 dpi.
    pub scale: f64,
    /// Colour the page is composited onto.
    pub background: Rgba8,
    /// Whether to simulate overprinting (8.6.7; 11.7.4).
    pub overprint: OverprintPreview,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            scale: 1.0,
            background: Rgba8::WHITE,
            overprint: OverprintPreview::Auto,
        }
    }
}

impl RenderOptions {
    /// Options for a resolution in dots per inch.
    pub fn from_dpi(dpi: f64) -> Self {
        Self::default().with_scale(dpi / 72.0)
    }

    /// These options with `scale` device pixels per user-space unit.
    #[must_use]
    pub const fn with_scale(mut self, scale: f64) -> Self {
        self.scale = scale;
        self
    }

    /// These options compositing onto `background`.
    #[must_use]
    pub const fn with_background(mut self, background: Rgba8) -> Self {
        self.background = background;
        self
    }

    /// These options with the overprint `preview` policy.
    #[must_use]
    pub const fn with_overprint(mut self, preview: OverprintPreview) -> Self {
        self.overprint = preview;
        self
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
