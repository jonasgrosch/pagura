//! Bounds of the sRGB transfer function found without `powf`.
//!
//! [`super::encode`] raises a linear value to `1/2.4` with the platform's
//! `powf`. Where only a rounding of the encoded value matters, bounds that
//! certainly hold the encoded value decide that rounding whenever both
//! bounds round alike, which is nearly always.
//!
//! A table and a short series estimate `p ≈ v^(5/12)`, and the estimate is
//! checked against `p^12 = v^5` in floating point. With every rounding error
//! of the check accounted for, a passing check puts `p` within `2^-39.4` of
//! the exact power. `powf` is assumed to be within `2^-44` (256 units in the
//! last place) of the exact power of its operands, which every C library is
//! by a wide margin (pow is within one unit in glibc, musl, Apple's libm and
//! the Windows CRT); its exponent `fl(1/2.4)` differs from `5/12` by less
//! than `2^-54`, which moves the power by less than `2^-52` for these
//! values. So `p · (1 ± 2^-39)` holds the `powf` result, and rounded
//! arithmetic, being monotone, keeps the bounds through the encoding.
//!
//! The estimate's latency, not its instruction count, decides its cost in a
//! per-pixel colour chain: the series is short and evaluated in Estrin's
//! form, and the check runs beside the bounds, behind a branch that passes.

use super::{LINEAR_LIMIT, affine, encode};

/// Bounds `[low, high]` of [`encode`]`(v)`, for a linear value `v` in
/// `0..=1`, found without `powf`; `None` where the estimate does not pass
/// its check (never for values in `0..=1`, by the table's accuracy) or `v` is
/// not in `0..=1`.
#[inline]
pub fn encode_bounds(v: f64) -> Option<[f64; 2]> {
    if v <= LINEAR_LIMIT || v == 1.0 {
        // No power to bound: `pow(1, y)` is 1 exactly in every C library
        // (C Annex F), so `encode(1)` is `affine(1)`.
        let exact = if v == 1.0 { affine(1.0) } else { encode(v) };
        return Some([exact, exact]);
    }
    let p = estimate(v)?;
    // The check: `p^12` within `v^5 · (1 ± TOLERANCE)`. The products round
    // 11 and 4 times, the bounds once more each, so a pass gives
    // `|p^12 / v^5 − 1| ≤ TOLERANCE + 17u` and `|p / v^(5/12) − 1| ≤
    // (TOLERANCE + 17u) / 11 < 2^-39.4` (u = 2^-53). No product leaves
    // the normal range: `v > 2^-9`.
    let p4 = (p * p) * (p * p);
    let p12 = (p4 * p4) * p4;
    let v4 = (v * v) * (v * v);
    let v5 = v4 * v;
    if !(p12 >= v5 * (1.0 - TOLERANCE) && p12 <= v5 * (1.0 + TOLERANCE)) {
        return None;
    }
    // `2^-39.4 + 2^-44 + 2^-52 < 2^-39 − u`, the margin after rounding.
    Some([affine(p * (1.0 - MARGIN)), affine(p * (1.0 + MARGIN))])
}

/// The check's relative tolerance on `p^12`.
const TOLERANCE: f64 = 1.0 / (1_u64 << 36) as f64;

/// The bounds' relative margin around `p`.
const MARGIN: f64 = 1.0 / (1_u64 << 39) as f64;

/// The mantissa bits of an `f64`.
const MANTISSA: u64 = (1 << 52) - 1;

/// Mantissa segments: the top bits that select one.
const SEGMENT_BITS: u32 = 8;

/// `v^(5/12)` to about `2^-40.5` for `v` in `2^-9..1`, or `None` outside
/// it: `2^(5e/12)` for the exponent `e`, `c^(5/12)` for the centre `c` of
/// the mantissa's segment, and the binomial series of `(1 + x)^(5/12)` to
/// `x^3` for the mantissa `m = c · (1 + x)`, `|x| ≤ 2^-9`.
#[inline]
fn estimate(v: f64) -> Option<f64> {
    let bits = v.to_bits();
    let exponent = usize::try_from(bits >> 52).ok()?.checked_sub(1014)?;
    let scale = *SCALES.get(exponent)?;
    let segment = usize::try_from((bits & MANTISSA) >> (52 - SEGMENT_BITS)).ok()?;
    let &(inverse, root) = SEGMENTS.get(segment)?;
    let mantissa = f64::from_bits(bits & MANTISSA | 1.0_f64.to_bits());
    let x = mantissa * inverse - 1.0;
    let [a1, a2, a3] = SERIES;
    let series = (1.0 + a1 * x) + (x * x) * (a2 + a3 * x);
    Some((scale * root) * series)
}

/// The binomial coefficients `(5/12 choose k)`, `k = 1..=3`.
const SERIES: [f64; 3] = {
    let mut terms = [0.0; 3];
    let mut term = 1.0;
    let mut k = 0;
    while k < 3 {
        term *= (5.0 / 12.0 - k as f64) / (k + 1) as f64;
        terms[k] = term;
        k += 1;
    }
    terms
};

/// `2^(5e/12)` for the exponents `e = -9..=-1` of values in `2^-9..1`.
const SCALES: [f64; 9] = {
    let mut scales = [0.0; 9];
    let mut i = 0;
    while i < 9 {
        scales[i] = power(1.0 / (1_u64 << (9 - i)) as f64);
        i += 1;
    }
    scales
};

/// `(1 / c, c^(5/12))` for the centre `c` of each mantissa segment.
const SEGMENTS: [(f64, f64); 1 << SEGMENT_BITS] = {
    let count = 1 << SEGMENT_BITS;
    let mut segments = [(0.0, 0.0); 1 << SEGMENT_BITS];
    let mut i = 0;
    while i < count {
        let centre = 1.0 + (i as f64 + 0.5) / count as f64;
        segments[i] = (1.0 / centre, power(centre));
        i += 1;
    }
    segments
};

/// `x^(5/12)` for `x` in `2^-9..2`, by Newton's method on `y^12 = x^5`
/// from above the root (where it decreases to the root), for the tables.
const fn power(x: f64) -> f64 {
    let target = x * x * x * x * x;
    let mut y = if x > 1.0 { x } else { 1.0 };
    let mut i = 0;
    while i < 80 {
        let y2 = y * y;
        let y11 = y2 * y2 * y2 * y2 * y2 * y;
        y -= (y11 * y - target) / (12.0 * y11);
        i += 1;
    }
    y
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Values across every exponent and segment, the ends of the range and
    /// the encoding's threshold.
    fn values() -> impl Iterator<Item = f64> {
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        let random = std::iter::repeat_with(move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 11) as f64 / (1_u64 << 53) as f64
        });
        let above_limit = f64::from_bits(LINEAR_LIMIT.to_bits() + 1);
        let below_one = f64::from_bits(1.0_f64.to_bits() - 1);
        let edges = [0.0, LINEAR_LIMIT, above_limit, 0.5, below_one, 1.0];
        edges.into_iter().chain(random.take(200_000))
    }

    #[test]
    fn bounds_hold_the_encoded_value() {
        for v in values() {
            let [low, high] = encode_bounds(v).unwrap_or_else(|| panic!("{v} not bounded"));
            let exact = encode(v);
            assert!(low <= exact && exact <= high, "{v}: {low} {exact} {high}");
        }
    }

    #[test]
    fn bounds_are_tight() {
        for v in values().filter(|&v| v > LINEAR_LIMIT) {
            let [low, high] = encode_bounds(v).unwrap();
            assert!(high - low <= high * 1e-11, "{v}: {low} {high}");
        }
    }

    #[test]
    fn values_outside_the_unit_interval_are_not_bounded() {
        for v in [f64::NAN, 1.5, f64::INFINITY] {
            assert_eq!(encode_bounds(v), None);
        }
    }

    #[test]
    fn the_tables_estimate_the_power_closely() {
        for v in values().filter(|&v| v > 1.0 / 512.0 && v < 1.0) {
            let p = estimate(v).unwrap();
            let exact = v.powf(5.0 / 12.0);
            assert!((p / exact - 1.0).abs() < 1e-12, "{v}: {p} {exact}");
        }
    }
}
