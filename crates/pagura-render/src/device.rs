//! Device helpers that `pagura-raster` is gaining as `Path::rect` and
//! `quantize_unit` (convention 11). Render keeps exactly one copy of each
//! here until then, so adopting them is a one-line change.

use pagura_core::Rect;
use pagura_raster::{Path, PathBuilder};

/// A closed rectangle subpath through the corners of `rect` (8.5.2.1).
pub(crate) fn rectangle(rect: Rect) -> Path {
    let mut path = PathBuilder::new();
    path.rect(rect.x0(), rect.y0(), rect.width(), rect.height());
    path.finish()
}

/// The unit square an image occupies in image space (8.9.4).
pub(crate) fn unit_square() -> Path {
    let mut path = PathBuilder::new();
    path.rect(0.0, 0.0, 1.0, 1.0);
    path.finish()
}

/// Quantizes a unit-interval value (alpha or a colour component) to
/// 0..=255, rounding half up; out-of-range values are clamped and NaN maps
/// to 0. The guard keeps analytic half-step ties rounding up although
/// cancellation can perturb them by far less than 1e-9 of a level.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a float-to-integer `as` truncates toward zero, saturates and maps NaN to 0"
)]
pub(crate) fn quantize_unit(value: f64) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0 + 0.5 + 1e-9) as u8
}
