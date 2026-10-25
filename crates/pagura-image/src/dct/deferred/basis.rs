//! The reference transform (T.81 A.3.3) in binary64, in its addition order.
//!
//! The basis values are frozen so that no target's math library can move a
//! rounding decision: `VALUES[u][x]` is `C(u) / 2 · cos((2x + 1)uπ / 16)`.
const VALUES: [[f64; 8]; 8] = [
    [
        f64::from_bits(0x3fd6_a09e_667f_3bcd),
        f64::from_bits(0x3fd6_a09e_667f_3bcd),
        f64::from_bits(0x3fd6_a09e_667f_3bcd),
        f64::from_bits(0x3fd6_a09e_667f_3bcd),
        f64::from_bits(0x3fd6_a09e_667f_3bcd),
        f64::from_bits(0x3fd6_a09e_667f_3bcd),
        f64::from_bits(0x3fd6_a09e_667f_3bcd),
        f64::from_bits(0x3fd6_a09e_667f_3bcd),
    ],
    [
        f64::from_bits(0x3fdf_6297_cff7_5cb0),
        f64::from_bits(0x3fda_9b66_290e_a1a3),
        f64::from_bits(0x3fd1_c73b_39ae_68c9),
        f64::from_bits(0x3fb8_f8b8_3c69_a60d),
        f64::from_bits(0xbfb8_f8b8_3c69_a608),
        f64::from_bits(0xbfd1_c73b_39ae_68c6),
        f64::from_bits(0xbfda_9b66_290e_a1a4),
        f64::from_bits(0xbfdf_6297_cff7_5cb0),
    ],
    [
        f64::from_bits(0x3fdd_906b_cf32_8d46),
        f64::from_bits(0x3fc8_7de2_a6ae_a964),
        f64::from_bits(0xbfc8_7de2_a6ae_a962),
        f64::from_bits(0xbfdd_906b_cf32_8d46),
        f64::from_bits(0xbfdd_906b_cf32_8d47),
        f64::from_bits(0xbfc8_7de2_a6ae_a96d),
        f64::from_bits(0x3fc8_7de2_a6ae_a967),
        f64::from_bits(0x3fdd_906b_cf32_8d44),
    ],
    [
        f64::from_bits(0x3fda_9b66_290e_a1a3),
        f64::from_bits(0xbfb8_f8b8_3c69_a608),
        f64::from_bits(0xbfdf_6297_cff7_5cb0),
        f64::from_bits(0xbfd1_c73b_39ae_68c8),
        f64::from_bits(0x3fd1_c73b_39ae_68c5),
        f64::from_bits(0x3fdf_6297_cff7_5cb0),
        f64::from_bits(0x3fb8_f8b8_3c69_a61d),
        f64::from_bits(0xbfda_9b66_290e_a1a2),
    ],
    [
        f64::from_bits(0x3fd6_a09e_667f_3bcd),
        f64::from_bits(0xbfd6_a09e_667f_3bcc),
        f64::from_bits(0xbfd6_a09e_667f_3bce),
        f64::from_bits(0x3fd6_a09e_667f_3bcb),
        f64::from_bits(0x3fd6_a09e_667f_3bce),
        f64::from_bits(0xbfd6_a09e_667f_3bc5),
        f64::from_bits(0xbfd6_a09e_667f_3bc9),
        f64::from_bits(0x3fd6_a09e_667f_3bc4),
    ],
    [
        f64::from_bits(0x3fd1_c73b_39ae_68c9),
        f64::from_bits(0xbfdf_6297_cff7_5cb0),
        f64::from_bits(0x3fb8_f8b8_3c69_a60c),
        f64::from_bits(0x3fda_9b66_290e_a1a5),
        f64::from_bits(0xbfda_9b66_290e_a1a2),
        f64::from_bits(0xbfb8_f8b8_3c69_a602),
        f64::from_bits(0x3fdf_6297_cff7_5cb2),
        f64::from_bits(0xbfd1_c73b_39ae_68c2),
    ],
    [
        f64::from_bits(0x3fc8_7de2_a6ae_a964),
        f64::from_bits(0xbfdd_906b_cf32_8d47),
        f64::from_bits(0x3fdd_906b_cf32_8d44),
        f64::from_bits(0xbfc8_7de2_a6ae_a965),
        f64::from_bits(0xbfc8_7de2_a6ae_a971),
        f64::from_bits(0x3fdd_906b_cf32_8d46),
        f64::from_bits(0xbfdd_906b_cf32_8d43),
        f64::from_bits(0x3fc8_7de2_a6ae_a95f),
    ],
    [
        f64::from_bits(0x3fb8_f8b8_3c69_a60d),
        f64::from_bits(0xbfd1_c73b_39ae_68c8),
        f64::from_bits(0x3fda_9b66_290e_a1a5),
        f64::from_bits(0xbfdf_6297_cff7_5cb2),
        f64::from_bits(0x3fdf_6297_cff7_5cb0),
        f64::from_bits(0xbfda_9b66_290e_a1a1),
        f64::from_bits(0x3fd1_c73b_39ae_68c2),
        f64::from_bits(0xbfb8_f8b8_3c69_a616),
    ],
];

/// Reference evaluation for individual samples of a dequantized block.
pub(super) struct Reference {
    basis: [[f64; 8]; 8],
}

/// The byte of a reconstructed sample, rounded and clamped.
fn sample(value: f64) -> u8 {
    pagura_core::convert::round_u8(value)
}

impl Reference {
    pub(super) fn new() -> Self {
        Self { basis: VALUES }
    }

    /// The sample of a block whose AC coefficients are zero, with the
    /// reference's two multiplications.
    pub(super) fn dc(&self, dc: i32) -> u8 {
        sample(f64::from(dc) * self.basis[0][0] * self.basis[0][0] + 128.0)
    }

    /// Recomputes the samples selected by `mask` (bit `8y + x`) and returns
    /// the arithmetic operations used.
    pub(super) fn refine(
        &self,
        coefficients: &[i32; 64],
        mask: u64,
        output: &mut [[u8; 8]; 8],
    ) -> usize {
        let mut work = 0;
        for x in 0..8 {
            if mask & (0x0101_0101_0101_0101u64 << x) == 0 {
                continue;
            }
            let horizontal = std::array::from_fn::<_, 8, _>(|v| {
                let mut sum = 0.0;
                for u in 0..8 {
                    sum += f64::from(coefficients[v * 8 + u]) * self.basis[u][x];
                }
                sum
            });
            work += 64;
            for (y, row) in output.iter_mut().enumerate() {
                if mask & (1u64 << (y * 8 + x)) != 0 {
                    let mut value = 0.0;
                    for (&h, basis) in horizontal.iter().zip(&self.basis) {
                        value += h * basis[y];
                    }
                    row[x] = sample(value + 128.0);
                    work += 8;
                }
            }
        }
        work
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn frozen_values_are_the_cosine_basis() {
        for (u, row) in super::VALUES.iter().enumerate() {
            for (x, &value) in row.iter().enumerate() {
                let expected = if u == 0 { 0.5f64.sqrt() } else { 1.0 }
                    * (((2 * x + 1) * u) as f64 * std::f64::consts::PI / 16.0).cos()
                    * 0.5;
                assert!((value - expected).abs() < 1e-15, "({u},{x})");
            }
        }
    }
}
