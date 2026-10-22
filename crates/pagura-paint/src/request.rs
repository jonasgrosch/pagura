//! What a render asks for.

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
    /// Default user space to device pixels
    /// (`PageSpace::device_transform`).
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
}
