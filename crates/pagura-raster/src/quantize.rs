//! Conversions between unit-interval values and 8-bit device values.

/// Added in units of the 8-bit scale before rounding. Analytic coverage of a
/// half pixel step lands exactly on a rounding tie, which cancellation in
/// translated geometry can perturb by far less than this guard (under
/// 4e-12 of a pixel's area); the guard keeps such ties rounding up.
const TIE_GUARD: f64 = 1e-9;

/// Quantizes a unit-interval value (coverage, alpha or a color component)
/// to 0..=255, rounding half up. Values outside `0..=1` are clamped and NaN
/// maps to 0.
///
/// This is the single float-to-byte conversion of the workspace
/// (CONTRIBUTING.md convention 11), so equal values quantize equally in
/// masks, colors and composited pixels.
///
/// ```
/// use pagura_raster::quantize_unit;
///
/// assert_eq!(quantize_unit(0.5), 128);
/// assert_eq!(quantize_unit(1.0 / 255.0), 1);
/// assert_eq!(quantize_unit(-2.0), 0);
/// assert_eq!(quantize_unit(f64::NAN), 0);
/// ```
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a float-to-integer `as` truncates toward zero and maps NaN to 0; \
              the clamped level is below 256, so the byte needs no saturation"
)]
pub fn quantize_unit(value: f64) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0 + 0.5 + TIE_GUARD) as u32 as u8
}

/// `round(a × b / 255)`: the product of two 8-bit unit values.
pub(crate) fn multiply(a: u8, b: u8) -> u8 {
    // Exact for every product of two bytes (checked exhaustively below);
    // 255 is odd, so no product lies halfway between two levels.
    let value = u32::from(a) * u32::from(b) + 128;
    u8::try_from((value + (value >> 8)) >> 8).unwrap_or(u8::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantize_rounds_half_up_clamps_and_maps_nan_to_zero() {
        // The formula this replaces, kept as an independent reference.
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "reference formula, clamped to 0..=255"
        )]
        let reference = |v: f64| (v.clamp(0.0, 1.0) * 255.0 + 0.5 + 1e-9).floor().min(255.0) as u8;
        for step in 0..=1_000_000_u32 {
            let value = f64::from(step) / 1_000_000.0;
            assert_eq!(quantize_unit(value), reference(value), "{value}");
        }
        for level in 0..=255_u8 {
            let exact = f64::from(level) / 255.0;
            assert_eq!(quantize_unit(exact), level);
            // Half steps round up, also just below the tie.
            let tie = (f64::from(level) + 0.5) / 255.0;
            assert_eq!(quantize_unit(tie), level.saturating_add(1));
            assert_eq!(quantize_unit(tie - 1e-13), level.saturating_add(1));
            assert_eq!(quantize_unit(tie - 1e-9), level);
        }
        for (value, expected) in [
            (-1.0, 0),
            (f64::NEG_INFINITY, 0),
            (f64::NAN, 0),
            (2.0, 255),
            (f64::INFINITY, 255),
        ] {
            assert_eq!(quantize_unit(value), expected, "{value}");
        }
    }

    #[test]
    fn multiply_rounds_every_product_exactly() {
        for a in 0..=255_u8 {
            for b in 0..=255_u8 {
                let expected = (f64::from(a) * f64::from(b) / 255.0).round();
                assert_eq!(f64::from(multiply(a, b)), expected, "{a} × {b}");
            }
        }
    }
}
