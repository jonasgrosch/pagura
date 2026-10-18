//! Separable T.81 A.3.3 IDCT with unchanged f64 rounding.
//!
//! The basis is frequency-major: each coefficient updates eight independent
//! sample lanes. This lets the compiler vectorize across samples without
//! reassociating a sum. Both passes retain the reference's increasing-frequency
//! addition order and separate multiplications, including exact rounding ties.
use super::Sample;
#[cfg(test)]
use super::sample;

pub(super) struct Idct {
    basis: [[f64; 8]; 8],
}

impl Idct {
    pub(super) fn new() -> Self {
        let mut basis = [[0.0; 8]; 8];
        for (u, row) in basis.iter_mut().enumerate() {
            for (x, value) in row.iter_mut().enumerate() {
                *value = if u == 0 { 0.5f64.sqrt() } else { 1.0 }
                    * (((2 * x + 1) * u) as f64 * std::f64::consts::PI / 16.0).cos()
                    * 0.5;
            }
        }
        Self { basis }
    }

    pub(super) fn block<T: Sample>(&self, coefficients: &[f64; 64]) -> [[T; 8]; 8] {
        // Keep the two reference multiplications: replacing them by /8 moves
        // negative DC values across exact half ties in the existing decoder.
        if coefficients[1..].iter().all(|&v| v == 0.0) {
            return [[T::from_idct(coefficients[0] * self.basis[0][0] * self.basis[0][0]); 8]; 8];
        }
        let mut horizontal = [[0.0f64; 8]; 8];
        for (input, output) in coefficients.as_chunks::<8>().0.iter().zip(&mut horizontal) {
            for (&value, basis) in input.iter().zip(&self.basis) {
                for (out, &weight) in output.iter_mut().zip(basis) {
                    *out += value * weight;
                }
            }
        }
        let mut vertical = [[0.0f64; 8]; 8];
        for (input, basis) in horizontal.iter().zip(&self.basis) {
            for (output, &weight) in vertical.iter_mut().zip(basis) {
                for (out, &value) in output.iter_mut().zip(input) {
                    *out += value * weight;
                }
            }
        }
        vertical.map(|row| row.map(T::from_idct))
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
