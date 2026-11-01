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
    // `2^-39.4 + 2^-44 + 2^-52 < 2^-39 − u`, the margin after rounding.
    power_bounds(v, MARGIN)
}

/// Bounds `[low, high]` of [`encode`]`(v)` as [`encode_bounds`] gives, from
/// the estimate alone, without checking it: its error is bounded ahead of
/// time, which makes these bounds cheaper and no wider.
///
/// The estimate `p` of `v^(5/12)` is within `2^-40.5` of it: the series
/// leaves out terms below `|(5/12 choose 4)|·|x|^4 / (1 − |x|) < 2^-40.59`
/// for `|x| ≤ 2^-9`; the tables' entries are within `2^-51` of their values
/// (verified for every entry by the tests), `x` within `2u`, and evaluating
/// the series and the product rounds six more times. With `powf` within
/// `2^-44 + 2^-52` of `v^(5/12)` (see the module), `powf / p` lies within
/// `ε < 2^-40.35` of 1. The margin `2^-40` exceeds `ε + u`, so
/// `affine(p · (1 ∓ 2^-40))`, whose factors are exact, hold the exact
/// path's value.
#[inline]
pub fn encode_estimate(v: f64) -> Option<[f64; 2]> {
    if v <= LINEAR_LIMIT || v == 1.0 {
        let exact = if v == 1.0 { affine(1.0) } else { encode(v) };
        return Some([exact, exact]);
    }
    let p = estimate(v)?;
    Some([
        affine(p * (1.0 - ESTIMATE_MARGIN)),
        affine(p * (1.0 + ESTIMATE_MARGIN)),
    ])
}

/// [`encode_estimate`]'s relative margin around the estimate.
const ESTIMATE_MARGIN: f64 = 1.0 / (1_u64 << 40) as f64;

/// Bounds `[low, high]` of `affine(powf(w, 1/2.4))`, the power branch of
/// [`encode`], for every linear value `w` whose power `w^(5/12)` lies
/// within relative distance `spread` of `v^(5/12)`, where
/// `margin ≥ (2^-39 + spread) · (1 + 2^-40)` and `spread ≤ 2^-10`; `None`
/// where the estimate of `v^(5/12)` does not pass its check (never for `v`
/// in `2^-9..1`).
///
/// The estimate `p` is within `2^-39.4` of `v^(5/12)` and `powf(w)` within
/// `2^-44 + 2^-52` of `w^(5/12)`, so `powf(w) / p` lies within
/// `2^-39.4 + 2^-43.9 + spread` of 1 (cross terms included), which stays
/// below `margin − 2u` for the two roundings of `p · (1 ∓ margin)`.
/// [`encode_bounds`] takes `w = v` with `margin = 2^-39`, for which
/// `1 ∓ margin` is exact.
pub(crate) fn power_bounds(v: f64, margin: f64) -> Option<[f64; 2]> {
    let p = root(v)?;
    Some([affine(p * (1.0 - margin)), affine(p * (1.0 + margin))])
}

/// `v^(5/12)` within relative distance `2^-39.4`, or `None` where the
/// estimate does not pass its check (never for `v` in `2^-9..1`).
#[inline]
fn root(v: f64) -> Option<f64> {
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
    (p12 >= v5 * (1.0 - TOLERANCE) && p12 <= v5 * (1.0 + TOLERANCE)).then_some(p)
}

/// A cubic `k₀ + k₁·τ + k₂·τ² + k₃·τ³` in `τ = x − centre`, with its centre
/// and a radius `R`, such that for every `x` in `low..=high` and every
/// linear value `w` within `r` of the line `a + b·x` (as real numbers),
/// where every such `w` is at least `floor` and the exact path takes the
/// power branch,
/// the cubic evaluated in rounded arithmetic (Horner's rule, `τ` rounded)
/// and its ends `v ∓ R` (rounded) enclose `affine(powf(w, 1/2.4))`, the
/// power branch of [`encode`]. `None` where the estimate of the centre's
/// power fails or `floor` is not positive.
///
/// With `F(w) = 1.055·w^α − 0.055`, `α = 5/12`, `w_c` the rounded value of
/// the line at the centre and `y = b·τ`:
///
/// - the exact path's value is within `2^-42` of `F(w)`: `powf` is within
///   `2^-44 + 2^-52` of `w^α ≤ 1` (see the module), and the affine map
///   rounds twice;
/// - `|F(w) − F(a + b·x)| ≤ M₁·r` and `|F(a + b·x) − F(w_c + y)| ≤ M₁·s`,
///   where `s` bounds the rounding of `w_c` and `M₁` bounds `|F'| =
///   1.055·α·w^α / w` over every value involved, all at least `w_lo` and
///   with `w^α` at most `p·(1 + (w − w_c)/w_c)`, raised past `p`'s error;
/// - Taylor's theorem: `F(w_c + y)` is within `M₄·Y⁴/24` of its cubic
///   `Σ F⁽ʲ⁾(w_c)·yʲ/j!` for `|y| ≤ Y = |b|·h`, `h` the half width, where
///   `M₄` bounds `|F''''(w)| = 1.055·|α(α−1)(α−2)(α−3)|·w^α / w⁴`, which
///   falls with `w`;
/// - the coefficients `kⱼ` are those of the cubic in `τ` computed from the
///   checked estimate `p` of `w_c^α` (within `2^-39.4`) in at most ten
///   roundings, so `Σ|kⱼ − exact|·hʲ ≤ 2^-39·(1 + S)`, `S = Σ|kⱼ|·hʲ`;
/// - Horner's rule rounds six times and `τ` once more, within `10u·S`, and
///   the ends `v ∓ R` round within `2^-50` (`|v| ≤ 2`).
///
/// The radius sums these bounds, each computed in a few roundings, and is
/// raised by `2^-40` of itself past them.
pub(crate) fn power_cubic(
    [a, b]: [f64; 2],
    r: f64,
    floor: f64,
    [low, high]: [f64; 2],
) -> Option<([f64; 4], f64, f64)> {
    const ALPHA: f64 = 5.0 / 12.0;
    const U: f64 = f64::EPSILON / 2.0;
    let centre = low + 0.5 * (high - low);
    let h = ((high - centre).max(centre - low)) * (1.0 + 4.0 * U);
    let wc = a + b * centre;
    let shift = 4.0 * U * (a.abs() + (b * centre).abs());
    let span = b.abs() * h * (1.0 + 4.0 * U);
    // Every value involved is at least `w_lo` and at most `w_hi ≤ 1`.
    let w_lo = floor.min((wc - span - shift) * (1.0 - 4.0 * U));
    if !(w_lo > 0.0 && wc > 0.0 && h.is_finite() && span.is_finite()) {
        return None;
    }
    let p = root(wc)?;
    // `w^α ≤ w_c^α·(1 + (w − w_c)/w_c)` for `w ≥ w_c`.
    let top = p * (1.0 + 2.0 * TOLERANCE) * (1.0 + (span + shift + r) / wc);
    let q = p / wc;
    let k = [
        affine(p),
        (1.055 * ALPHA) * q * b,
        (1.055 * ALPHA * (ALPHA - 1.0) / 2.0) * (q / wc) * (b * b),
        (1.055 * ALPHA * (ALPHA - 1.0) * (ALPHA - 2.0) / 6.0) * (q / wc / wc) * (b * b * b),
    ];
    let sum = k[0].abs() + h * (k[1].abs() + h * (k[2].abs() + h * k[3].abs()));
    let m1 = 1.055 * ALPHA * top / w_lo;
    let m4 = 1.055 * (ALPHA * (1.0 - ALPHA) * (2.0 - ALPHA) * (3.0 - ALPHA)) * top
        / ((w_lo * w_lo) * (w_lo * w_lo));
    let taylor = m4 * ((span * span) * (span * span)) / 24.0;
    let radius = (1.0 / (1_u64 << 42) as f64
        + m1 * (r + shift)
        + taylor
        + (1.0 + sum) / (1_u64 << 39) as f64
        + 10.0 * U * sum)
        * (1.0 + 1.0 / (1_u64 << 40) as f64)
        + 1.0 / (1_u64 << 50) as f64;
    (k.iter().all(|c| c.is_finite()) && radius.is_finite()).then_some((k, centre, radius))
}

/// The check's relative tolerance on `p^12`.
const TOLERANCE: f64 = 1.0 / (1_u64 << 36) as f64;

/// The bounds' relative margin around `p`.
pub(crate) const MARGIN: f64 = 1.0 / (1_u64 << 39) as f64;

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
    fn cubics_enclose_the_encoded_values_of_their_lines() {
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 11) as f64 / (1_u64 << 53) as f64
        };
        let (mut cubics, mut narrow) = (0, 0);
        for _ in 0..20_000 {
            // A piece `low..=high` whose line stays above the linear segment.
            let (low, width) = (next() * 4.0 - 2.0, next() * next() * next() * 0.01);
            let high = low + width;
            let (w0, w1) = (LINEAR_LIMIT + next() * (1.0 - LINEAR_LIMIT), next());
            let w1 = LINEAR_LIMIT + (w1 * width * 4.0).min(1.0 - w0) + (w0 - LINEAR_LIMIT);
            let b = if width > 0.0 { (w1 - w0) / width } else { 0.0 };
            let a = w0 - b * low;
            let r = 1e-15 * next();
            let floor = (w0.min(w1) - 4.0 * r) * (1.0 - 1e-12);
            if floor <= LINEAR_LIMIT {
                continue;
            }
            let Some((k, centre, radius)) = power_cubic([a, b], r, floor, [low, high]) else {
                continue;
            };
            cubics += 1;
            narrow += usize::from(radius <= 1.0 / (1_u64 << 36) as f64);
            for i in 0..=8 {
                let x = (low + width * f64::from(i) / 8.0).clamp(low, high);
                let w = (a + b * x) + r * (2.0 * next() - 1.0) * 0.5;
                let t = x - centre;
                let v = k[0] + t * (k[1] + t * (k[2] + t * k[3]));
                let exact = encode(w);
                assert!(
                    v - radius <= exact && exact <= v + radius,
                    "x {x} w {w}: {exact} not within {radius} of {v}"
                );
            }
        }
        assert!(
            cubics > 15_000 && narrow * 2 > cubics,
            "{cubics} cubics, {narrow} narrow"
        );
    }

    /// `a · b` as a double-double `(high, low)`.
    fn product((ah, al): (f64, f64), (bh, bl): (f64, f64)) -> (f64, f64) {
        let high = ah * bh;
        let low = ah.mul_add(bh, -high) + (ah * bl + al * bh);
        let sum = high + low;
        (sum, low - (sum - high))
    }

    /// `x^n` as a double-double.
    fn power_dd(x: f64, n: u32) -> (f64, f64) {
        (1..n).fold((x, 0.0), |p, _| product(p, (x, 0.0)))
    }

    /// Every table entry is within `2^-51` of its value, as the error
    /// argument of [`encode_estimate`] needs: `y^12 / x^5` within `12·2^-51`
    /// of 1 in double-double arithmetic, whose own error is below `2^-90`,
    /// and each reciprocal correctly rounded.
    #[test]
    fn table_entries_are_within_two_units() {
        let close = |x: f64, y: f64| {
            let (y12, x5) = (power_dd(y, 12), power_dd(x, 5));
            let ratio = (y12.0 - x5.0 + (y12.1 - x5.1)) / x5.0;
            ratio.abs() <= 12.0 / (1_u64 << 51) as f64
        };
        for (i, &scale) in SCALES.iter().enumerate() {
            let x = 1.0 / (1_u64 << (9 - i)) as f64;
            assert!(close(x, scale), "scale {i}");
        }
        let count = SEGMENTS.len();
        for (i, &(inverse, root)) in SEGMENTS.iter().enumerate() {
            let centre = 1.0 + (i as f64 + 0.5) / count as f64;
            assert!(close(centre, root), "segment {i}");
            let unit = product((centre, 0.0), (inverse, 0.0));
            let error = (unit.0 - 1.0 + unit.1).abs();
            assert!(error <= 1.0 / (1_u64 << 53) as f64, "inverse {i}");
        }
    }

    #[test]
    fn estimated_bounds_hold_the_encoded_value_and_are_tight() {
        for v in values() {
            let [low, high] = encode_estimate(v).unwrap_or_else(|| panic!("{v} not bounded"));
            let exact = encode(v);
            assert!(low <= exact && exact <= high, "{v}: {low} {exact} {high}");
            assert!(high - low <= high * 5e-12, "{v}: {low} {high}");
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
