//! Conversions between number types that the standard library spells
//! awkwardly or slowly.
//!
//! Pagura supports targets whose `usize` is 32 or 64 bits wide (WebAssembly
//! and native). On those, `u32 → usize` and `usize → u64` never lose
//! information, but the standard library offers no `From` for them, which
//! leads to a mix of `as` casts and `try_from(..).unwrap_or(MAX)` fallbacks
//! that can never fire. These helpers are the one spelling.
//!
//! WebAssembly and baseline x86-64 have no instruction that rounds halves
//! away from zero, so `v.round()` is a call into the software libm there and
//! costs more than the pixel arithmetic around it. [`round_u8`],
//! [`round_u16`] and [`round_f32_u8`] give the integers of `v.round() as u8`
//! (or `u16`) from one addition and the truncating conversion, which every
//! target has.

const _: () = assert!(usize::BITS >= 32 && usize::BITS <= 64);

/// The largest `f64` below one half.
const BELOW_HALF: f64 = 0.499_999_999_999_999_94;
/// The largest `f32` below one half.
const BELOW_HALF_F32: f32 = 0.499_999_97;

/// Widens a `u32` to `usize`.
pub const fn u32_to_usize(value: u32) -> usize {
    value as usize
}

/// Widens a `usize` to `u64`.
pub const fn usize_to_u64(value: usize) -> u64 {
    value as u64
}

/// `value.round() as u8` for every `value`: the nearest byte with halves
/// away from zero, saturating, NaN as 0.
///
/// For `n ≤ value < n + 1`, the sum `value + (½ − ε)` reaches `n + 1`
/// exactly when `value ≥ n + ½`: from the half the sum rounds up to `n + 1`,
/// and below the half it stays below. Truncation then gives the nearest
/// integer, and the saturating conversion clamps as `as` does; negative
/// values and NaN give 0 either way.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the saturating conversion is the clamp"
)]
pub fn round_u8(value: f64) -> u8 {
    (value + BELOW_HALF) as u8
}

/// `value.round() as u16` for every `value`, as [`round_u8`] rounds.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the saturating conversion is the clamp"
)]
pub fn round_u16(value: f64) -> u16 {
    (value + BELOW_HALF) as u16
}

/// `value.round() as u8` for every `f32` `value`, as [`round_u8`] rounds.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the saturating conversion is the clamp"
)]
pub fn round_f32_u8(value: f32) -> u8 {
    (value + BELOW_HALF_F32) as u8
}

#[cfg(test)]
mod tests {
    use super::{
        BELOW_HALF, BELOW_HALF_F32, round_f32_u8, round_u8, round_u16, u32_to_usize, usize_to_u64,
    };

    #[test]
    fn widening_preserves_the_extremes() {
        assert_eq!(u32_to_usize(u32::MAX), 4_294_967_295);
        assert_eq!(usize_to_u64(usize::MAX), u64::try_from(usize::MAX).unwrap());
        assert_eq!(usize_to_u64(0), 0);
    }

    #[test]
    fn below_half_is_the_float_before_one_half() {
        assert_eq!(BELOW_HALF, 0.5_f64.next_down());
        assert_eq!(BELOW_HALF_F32, 0.5_f32.next_down());
    }

    #[test]
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the reference conversions"
    )]
    fn rounding_conversions_equal_round_then_as() {
        let mut wide = vec![
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::MAX,
            f64::MIN,
            -0.0,
            f64::MIN_POSITIVE,
            2.0_f64.powi(52),
            2.0_f64.powi(52) + 1.0,
            2.0_f64.powi(53) - 1.0,
        ];
        let mut narrow = vec![
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::MAX,
            f32::MIN,
            -0.0,
            f32::MIN_POSITIVE,
        ];
        for k in -3..=65_538 {
            let k = f64::from(k);
            for x in [k, k - 0.5, k + 0.5] {
                wide.extend([x, x.next_up(), x.next_down()]);
                let x = x as f32;
                narrow.extend([x, x.next_up(), x.next_down()]);
            }
        }
        wide.extend(
            (0..=u64::MAX)
                .step_by(0x0000_1000_0000_0F01)
                .map(f64::from_bits),
        );
        narrow.extend((0..=u32::MAX).step_by(4099).map(f32::from_bits));
        for x in wide {
            assert_eq!(round_u8(x), x.round() as u8, "{x}");
            assert_eq!(round_u16(x), x.round() as u16, "{x}");
        }
        for x in narrow {
            assert_eq!(round_f32_u8(x), x.round() as u8, "{x}");
        }
    }
}
