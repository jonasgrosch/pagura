//! Exact coverage of single fills, strokes and glyph placements outside a
//! scene, for differential tests of `pagura-raster`'s band rasterizer: the
//! same regions and quantization the reference renderer uses for marks and
//! clips.

use crate::area::Region;
use crate::flatten::glyph_region;
use crate::request::Quantization;
use crate::shapes::{self, Coverage};
use pagura_core::{Error, Limits, Result};
use pagura_interpret::Pen;
use pagura_raster::{
    Adjustment, DeviceRect, FillRule, Path, Point, Stroke, Transform, quantize_unit,
};

/// The region of a fill of `path` under `transform` and `rule`; an empty
/// filling region with a boundary becomes its hairline outline (10.7.4).
///
/// # Errors
///
/// Returns the geometry's `InvalidArgument` and `Limit` errors.
pub fn fill(path: &Path, transform: Transform, rule: FillRule, limits: &Limits) -> Result<Region> {
    shapes::fill(path, rule, transform, limits)
}

/// The region of a stroke of `path` (user space) with `stroke`, whose width
/// and dashes are measured in pen space, under `adjustment`; `ctm` maps pen
/// space to user space and `device` user space to device space.
///
/// # Errors
///
/// Returns `InvalidArgument` for a singular `ctm` and the stroker's errors.
pub fn stroke(
    path: &Path,
    stroke: &Stroke,
    ctm: Transform,
    adjustment: Adjustment,
    device: Transform,
    limits: &Limits,
) -> Result<Region> {
    let pen = Pen {
        stroke: stroke.clone(),
        transform: ctm,
        adjustment,
    };
    shapes::stroke(path, &pen, device, limits)
}

/// The region of a glyph outline placed at the device-space `origin`, its
/// points mapped by the linear map `linear` (glyph space to device pixels),
/// against its true curve where the renderer integrates a spline of it
/// (ADR 0004 §4).
///
/// # Errors
///
/// As [`fill`].
pub fn glyph(outline: &Path, linear: [f64; 4], origin: Point, limits: &Limits) -> Result<Region> {
    let [a, b, c, d] = linear;
    let linear = Transform::new(a, b, c, d, 0.0, 0.0);
    let reference = crate::glyph::reference(outline, linear, limits)?;
    glyph_region(outline, linear, origin, reference, limits)
}

/// The coverage bytes of the intersection of `regions` over `window`,
/// row-major: each pixel's exact covered area, quantized once.
///
/// # Errors
///
/// Returns a `Limit` error for a window whose pixels do not fit `usize`.
pub fn coverage(regions: &[Region], window: DeviceRect) -> Result<Vec<u8>> {
    let pixels = usize::try_from(window.width())
        .ok()
        .and_then(|w| w.checked_mul(usize::try_from(window.height()).ok()?))
        .ok_or(Error::limit("coverage window too large"))?;
    let coverage = Coverage::of(regions, window, Quantization::Bytes);
    let mut bytes = Vec::with_capacity(pixels);
    for y in window.y0..window.y1 {
        bytes.extend((window.x0..window.x1).map(|x| quantize_unit(coverage.get(x, y))));
    }
    Ok(bytes)
}
