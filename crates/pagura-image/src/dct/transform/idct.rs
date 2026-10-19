//! Separable T.81 A.3.3 IDCT with guarded f32 evaluation and f64 rounding.
//!
//! The basis is frequency-major: each coefficient updates eight independent
//! sample lanes. This lets the compiler vectorize across samples without
//! reassociating a sum. Both passes retain the reference's increasing-frequency
//! addition order and separate multiplications, including exact rounding ties.
use super::Sample;
#[cfg(test)]
use super::sample;

pub(in crate::dct) struct Idct {
    basis: [[f64; 8]; 8],
    narrow: [[f32; 8]; 8],
}

impl Idct {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "rounded basis has a bounded f32 error"
    )]
    pub(in crate::dct) fn new() -> Self {
        let mut basis = [[0.0; 8]; 8];
        for (u, row) in basis.iter_mut().enumerate() {
            for (x, value) in row.iter_mut().enumerate() {
                *value = if u == 0 { 0.5f64.sqrt() } else { 1.0 }
                    * (((2 * x + 1) * u) as f64 * std::f64::consts::PI / 16.0).cos()
                    * 0.5;
            }
        }
        Self {
            narrow: basis.map(|row| row.map(|value| value as f32)),
            basis,
        }
    }

    /// The one value of a block whose AC coefficients are zero.
    pub(super) fn constant<T: Sample>(&self, dc: f64) -> T {
        // Keep the two reference multiplications: replacing them by /8 moves
        // negative DC values across exact half ties in the existing decoder.
        T::from_idct(dc * self.basis[0][0] * self.basis[0][0])
    }

    pub(super) fn block<T: Sample>(&self, coefficients: &[f64; 64]) -> [[T; 8]; 8] {
        if coefficients[1..].iter().all(|&v| v == 0.0) {
            return [[self.constant(coefficients[0]); 8]; 8];
        }
        if let Some(values) = self.fast(coefficients) {
            return values.map(|row| row.map(|value| T::from_idct(f64::from(value))));
        }
        self.precise(coefficients).map(|row| row.map(T::from_idct))
    }

    fn precise(&self, coefficients: &[f64; 64]) -> [[f64; 8]; 8] {
        let mut horizontal = [[0.0f64; 8]; 8];
        for (input, output) in coefficients.as_chunks::<8>().0.iter().zip(&mut horizontal) {
            for (&value, basis) in input.iter().zip(&self.basis) {
                if value == 0.0 {
                    continue;
                }
                for (out, &weight) in output.iter_mut().zip(basis) {
                    *out += value * weight;
                }
            }
        }
        let mut vertical = [[0.0f64; 8]; 8];
        for (input, basis) in horizontal.iter().zip(&self.basis) {
            if input.iter().all(|&value| value == 0.0) {
                continue;
            }
            for (output, &weight) in vertical.iter_mut().zip(basis) {
                for (out, &value) in output.iter_mut().zip(input) {
                    *out += value * weight;
                }
            }
        }
        vertical
    }

    /// For u=2^-24, each term passes through at most 18 f32 roundings
    /// (coefficients are exact below 2^15): two basis narrowings, two
    /// products and two eight-term sums. |B[u,x]| <= 1/2 gives error <=
    /// gamma(18)*sum(|c|)/4. Including
    /// the f64 reference's roundings and native bias, this is strictly less
    /// than sum(|c|)*2^-21 for integral dequantized coefficients (sum >= 1).
    /// A uniform small bound is impossible at the allowed i32*u16 extremes;
    /// large blocks and intervals touching a half integer retain f64.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "both narrowing errors are covered by the interval bound"
    )]
    fn fast(&self, coefficients: &[f64; 64]) -> Option<[[f32; 8]; 8]> {
        // Positive integral partial sums are exact below 2^53. Independent
        // lanes avoid serializing the error bound before the transform.
        let mut magnitudes = [0.0f64; 8];
        for row in coefficients.as_chunks::<8>().0 {
            for (sum, &value) in magnitudes.iter_mut().zip(row) {
                *sum += value.abs();
            }
        }
        let magnitude: f64 = magnitudes.into_iter().sum();
        if magnitude > 32768.0 {
            return None;
        }
        // At most 1/64 native level; next_down makes the guard conservative.
        let threshold = (0.5 - magnitude * (1.0 / 2_097_152.0)) as f32;
        let threshold = threshold.next_down();
        let values = self.approximate(coefficients);
        let mut ambiguous = false;
        for row in &values {
            for &value in row {
                ambiguous |= (value - value.round()).abs() >= threshold;
            }
        }
        (!ambiguous).then_some(values)
    }

    #[expect(
        clippy::cast_possible_truncation,
        reason = "coefficient conversion is included in the error bound"
    )]
    fn approximate(&self, coefficients: &[f64; 64]) -> [[f32; 8]; 8] {
        let mut horizontal = [[0.0f32; 8]; 8];
        for (input, output) in coefficients.as_chunks::<8>().0.iter().zip(&mut horizontal) {
            for (&coefficient, basis) in input.iter().zip(&self.narrow) {
                let value = coefficient as f32;
                if value == 0.0 {
                    continue;
                }
                for (out, &weight) in output.iter_mut().zip(basis) {
                    *out += value * weight;
                }
            }
        }
        let mut vertical = [[0.0f32; 8]; 8];
        for (input, basis) in horizontal.iter().zip(&self.narrow) {
            if input.iter().all(|&value| value == 0.0) {
                continue;
            }
            for (output, &weight) in vertical.iter_mut().zip(basis) {
                for (out, &value) in output.iter_mut().zip(input) {
                    *out += value * weight;
                }
            }
        }
        vertical
    }

    #[cfg(test)]
    fn horizontal(&self, coefficients: &[f64; 64]) -> [[f64; 8]; 8] {
        let mut horizontal = [[0.0; 8]; 8];
        for (input, row) in coefficients.as_chunks::<8>().0.iter().zip(&mut horizontal) {
            for (x, value) in row.iter_mut().enumerate() {
                *value = input
                    .iter()
                    .enumerate()
                    .map(|(u, a)| a * self.basis[u][x])
                    .sum();
            }
        }
        horizontal
    }

    #[cfg(test)]
    fn reference(&self, coefficients: &[f64; 64]) -> [[u8; 8]; 8] {
        let horizontal = self.horizontal(coefficients);
        let mut block = [[0; 8]; 8];
        for (y, row) in block.iter_mut().enumerate() {
            for (x, out) in row.iter_mut().enumerate() {
                *out = sample(
                    horizontal
                        .iter()
                        .enumerate()
                        .map(|(v, row)| row[x] * self.basis[v][y])
                        .sum::<f64>()
                        + 128.0,
                );
            }
        }
        block
    }
}

#[cfg(test)]
mod tests {
    use super::Idct;

    #[test]
    fn single_axes_keep_every_reference_product_and_rounding_tie() {
        let idct = Idct::new();
        let mut state = 0x1234_abcdu32;
        for axis in [1, 8] {
            for _ in 0..4096 {
                let mut coefficients = [0.0; 64];
                for frequency in 0..8 {
                    state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    coefficients[frequency * axis] =
                        f64::from(i32::try_from((state >> 16) % 4096).unwrap() - 2048);
                }
                let reference = idct.precise(&coefficients);
                assert_eq!(
                    idct.block::<u8>(&coefficients),
                    reference.map(|r| r.map(<u8 as super::Sample>::from_idct))
                );
                assert_eq!(
                    idct.block::<u16>(&coefficients),
                    reference.map(|r| r.map(<u16 as super::Sample>::from_idct))
                );
            }
        }
    }

    #[test]
    fn bounded_f32_intervals_preserve_both_native_precisions() {
        let idct = Idct::new();
        let mut state = 0x73ab_190du32;
        let (mut accepted, mut rejected, mut raw_differences, mut max_error) = (0, 0, 0, 0.0f64);
        for block in 0..20_000 {
            let mut coefficients = [0.0; 64];
            for value in &mut coefficients {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                if block % 2 == 0 || state.is_multiple_of(8) {
                    *value = f64::from(i32::try_from((state >> 16) % 512).unwrap() - 256);
                }
            }
            let exact = idct.precise(&coefficients);
            let narrow = idct.approximate(&coefficients);
            let bound = coefficients.iter().map(|v| v.abs()).sum::<f64>() / 2_097_152.0;
            for (&reference, &approx) in exact.as_flattened().iter().zip(narrow.as_flattened()) {
                let error = (reference - f64::from(approx)).abs();
                assert!(error <= bound, "block {block}: {error} > {bound}");
                max_error = max_error.max(error);
                let different =
                    super::sample(reference + 128.0) != super::sample(f64::from(approx) + 128.0);
                raw_differences += usize::from(different);
            }
            if idct.fast(&coefficients).is_some() {
                accepted += 1;
            } else {
                rejected += 1;
            }
            assert_eq!(
                idct.block::<u8>(&coefficients),
                exact.map(|r| r.map(<u8 as super::Sample>::from_idct))
            );
            assert_eq!(
                idct.block::<u16>(&coefficients),
                exact.map(|r| r.map(<u16 as super::Sample>::from_idct))
            );
        }
        assert!(accepted > 10_000 && rejected > 0);
        eprintln!(
            "IDCT oracle: accepted {accepted}, fallback {rejected}, raw rounded differences {raw_differences}, max pre-round error {max_error}"
        );
    }

    #[test]
    fn coefficient_extremes_use_the_f64_reference() {
        let idct = Idct::new();
        let extreme = f64::from(i32::MAX) * f64::from(u16::MAX);
        let mut max_error = 0.0f64;
        for target in 0..64 {
            let (x, y) = (target % 8, target / 8);
            for amplitude in [1.0, 32768.0, extreme] {
                let coefficients = std::array::from_fn(|i| {
                    amplitude * (idct.basis[i % 8][x] * idct.basis[i / 8][y]).signum()
                });
                let exact = idct.precise(&coefficients);
                let approx = idct.approximate(&coefficients);
                let bound = coefficients.iter().map(|v| v.abs()).sum::<f64>() / 2_097_152.0;
                for (&reference, &narrow) in exact.as_flattened().iter().zip(approx.as_flattened())
                {
                    let error = (reference - f64::from(narrow)).abs();
                    assert!(error <= bound);
                    max_error = max_error.max(error);
                }
                if amplitude > 1.0 {
                    assert!(idct.fast(&coefficients).is_none());
                }
                assert_eq!(
                    idct.block::<u8>(&coefficients),
                    exact.map(|r| r.map(<u8 as super::Sample>::from_idct))
                );
                assert_eq!(
                    idct.block::<u16>(&coefficients),
                    exact.map(|r| r.map(<u16 as super::Sample>::from_idct))
                );
            }
        }
        eprintln!("unrestricted f32 extreme pre-round error: {max_error}");
    }

    #[test]
    fn quarter_frequency_integer_samples_follow_annex_a() {
        let idct = Idct::new();
        let mut coefficients = [0.0; 64];
        coefficients[0] = 8.0;
        coefficients[4] = 16.0;
        coefficients[32] = 24.0;
        coefficients[36] = 32.0;
        // At frequency 4, cos((2x+1)*pi/4) has signs +--++--+.
        // With these multiples of 8, A.3.3 gives integer samples:
        // 128 + 1 + 2*sx + 3*sy + 4*sx*sy, without rounding ties.
        let expected = [
            [138, 126, 126, 138, 138, 126, 126, 138],
            [124, 128, 128, 124, 124, 128, 128, 124],
            [124, 128, 128, 124, 124, 128, 128, 124],
            [138, 126, 126, 138, 138, 126, 126, 138],
            [138, 126, 126, 138, 138, 126, 126, 138],
            [124, 128, 128, 124, 124, 128, 128, 124],
            [124, 128, 128, 124, 124, 128, 128, 124],
            [138, 126, 126, 138, 138, 126, 126, 138],
        ];
        assert_eq!(idct.block::<u8>(&coefficients), expected);
    }

    #[test]
    fn dc_fill_keeps_reference_ties_and_clamping() {
        let idct = Idct::new();
        for dc in -4096..=4096 {
            let mut coefficients = [0.0; 64];
            coefficients[0] = f64::from(dc);
            assert_eq!(
                idct.block::<u8>(&coefficients),
                idct.reference(&coefficients),
                "DC {dc}"
            );
        }
    }

    #[test]
    fn sparse_dense_and_large_coefficients_keep_reference_bytes() {
        let idct = Idct::new();
        let mut state = 0x1234_5678u32;
        for block in 0..20_000 {
            let mut coefficients = [0.0; 64];
            for value in &mut coefficients {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let amplitude = if block % 4 == 0 { 1_000_000 } else { 4096 };
                if block % 2 == 0 || state.is_multiple_of(8) {
                    *value = f64::from(i32::try_from(state % amplitude).unwrap())
                        - f64::from(amplitude / 2);
                }
            }
            assert_eq!(
                idct.block::<u8>(&coefficients),
                idct.reference(&coefficients),
                "block {block}"
            );
        }
    }

    #[test]
    fn quarter_frequency_ties_keep_reference_bytes() {
        let idct = Idct::new();
        for dc in -64..64 {
            for ac in -32..32 {
                let mut coefficients = [0.0; 64];
                coefficients[0] = f64::from(dc * 8 + 4);
                coefficients[4] = f64::from(ac * 8);
                coefficients[32] = f64::from((ac + 1) * 8);
                coefficients[36] = f64::from((ac - 1) * 8);
                assert_eq!(
                    idct.block::<u8>(&coefficients),
                    idct.reference(&coefficients)
                );
            }
        }
    }
}
