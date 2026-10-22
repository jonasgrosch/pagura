//! What a render asks for.

use pagura_raster::{DeviceRect, Rgba8, Transform};

/// The four named fast parameters (ADR 0034). Each switches a kernel or
/// producer variant inside the one pipeline and can be enabled alone; none
/// is within the one-level contract of ADR 0004, and their combination is
/// held to ADR 0004's competitive gate.
///
/// A plain set of switches built with functional record update, so it is
/// not `#[non_exhaustive]`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "four independent switches, each tested alone and in combination"
)]
pub struct FastParams {
    /// Glyph coverage reused per origin bin, with coarser glyph chords.
    pub glyph_origin_bins: bool,
    /// f32 signed-area cells whose winding is folded without certification
    /// or resolution, and clip coverage multiplied into marks instead of
    /// joint geometry.
    pub approximate_coverage: bool,
    /// U8 storage for the page buffer of a page without group results,
    /// tiling cells or soft masks, instead of N16 or F32; every other buffer
    /// is F64.
    pub byte_storage: bool,
    /// Images decoded near their drawn size: JPEG cells and JPEG 2000
    /// resolution levels.
    pub reduced_image_decode: bool,
}

impl FastParams {
    /// Exact rendering: every parameter off.
    pub const EXACT: Self = Self {
        glyph_origin_bins: false,
        approximate_coverage: false,
        byte_storage: false,
        reduced_image_decode: false,
    };

    /// Competitive rendering: every parameter on.
    pub const COMPETITIVE: Self = Self {
        glyph_origin_bins: true,
        approximate_coverage: true,
        byte_storage: true,
        reduced_image_decode: true,
    };

    /// Whether every parameter is off.
    pub fn is_exact(self) -> bool {
        self == Self::EXACT
    }
}

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
    /// Fast parameters.
    pub fast: FastParams,
    /// Rows per band of the global grid; output does not depend on it.
    pub band_height: u32,
}

impl RenderRequest {
    /// The default band height.
    pub const BAND_HEIGHT: u32 = 32;

    /// A request for `region` of a page of `page` pixels under `transform`,
    /// exact, on white, with the default band height.
    pub fn new(transform: Transform, page: DeviceRect, region: DeviceRect) -> Self {
        Self {
            transform,
            page,
            region,
            background: Rgba8::WHITE,
            overprint: OverprintMode::Ignore,
            fast: FastParams::EXACT,
            band_height: Self::BAND_HEIGHT,
        }
    }
}
