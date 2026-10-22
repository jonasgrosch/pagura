//! What a render asks for.

use pagura_core::{Error, Result};
use pagura_document::Rotation;
use pagura_interpret::PageSpace;
use pagura_raster::{DeviceRect, Rgba8, Transform};

/// Whether overprinting is simulated (8.6.7, 11.7.4), already resolved from
/// the caller's preview policy and the document's print intent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum OverprintMode {
    /// Overprint flags are ignored.
    #[default]
    Ignore,
    /// Overprinting is simulated, with spot colorants as channels.
    Simulate,
}

/// One rendering of a scene.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct RenderRequest {
    /// Default user space to device pixels ([`Self::page`]).
    pub transform: Transform,
    /// The page's device pixels: `0..width × 0..height`. Marks are clipped
    /// to it.
    pub page: DeviceRect,
    /// The pixels to render, within `page`.
    pub region: DeviceRect,
    /// The colour the page is composited onto.
    pub background: Rgba8,
    /// Overprint simulation.
    pub overprint: OverprintMode,
    /// Rows per band of the global grid; output does not depend on it.
    pub band_height: u32,
}

impl RenderRequest {
    /// The default band height.
    pub const BAND_HEIGHT: u32 = 32;

    /// A request for `region` of a page of `page` pixels under `transform`,
    /// on white, with the default band height.
    pub fn new(transform: Transform, page: DeviceRect, region: DeviceRect) -> Self {
        Self {
            transform,
            page,
            region,
            background: Rgba8::WHITE,
            overprint: OverprintMode::Ignore,
            band_height: Self::BAND_HEIGHT,
        }
    }

    /// A request for the whole page of `space` at `scale` device pixels per
    /// default unit (8.3.2): the crop box's corner goes to the origin, y
    /// grows downwards, the page turns clockwise by `Rotate`, `UserUnit`
    /// scales it, and its size is rounded up to whole pixels.
    ///
    /// # Errors
    ///
    /// Returns `InvalidArgument` for a non-positive or non-finite scale and
    /// a `Limit` error when the size does not fit `u32`.
    pub fn page(space: &PageSpace, scale: f64) -> Result<Self> {
        if !(scale.is_finite() && scale > 0.0) {
            return Err(Error::invalid_argument("scale must be positive and finite"));
        }
        let r = space.crop;
        let rotation = match space.rotation {
            Rotation::None => Transform::new(1.0, 0.0, 0.0, -1.0, -r.x0(), r.y1()),
            Rotation::Clockwise90 => Transform::new(0.0, 1.0, 1.0, 0.0, -r.y0(), -r.x0()),
            Rotation::Clockwise180 => Transform::new(-1.0, 0.0, 0.0, 1.0, r.x1(), -r.y0()),
            Rotation::Clockwise270 => Transform::new(0.0, -1.0, -1.0, 0.0, r.y1(), r.x1()),
        };
        let factor = scale * space.user_unit;
        let (width, height) = match space.rotation {
            Rotation::None | Rotation::Clockwise180 => (r.width(), r.height()),
            Rotation::Clockwise90 | Rotation::Clockwise270 => (r.height(), r.width()),
        };
        let page = DeviceRect {
            x0: 0,
            y0: 0,
            x1: pixels(width * factor)?,
            y1: pixels(height * factor)?,
        };
        Ok(Self::new(
            rotation.then(Transform::scale(factor, factor)),
            page,
            page,
        ))
    }
}

/// Rounds a device extent up to whole pixels, ignoring floating-point noise
/// just above an integer.
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
        reason = "range checked above; the value is a positive integer"
    )]
    Ok(rounded as u32)
}
