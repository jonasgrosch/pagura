//! Drawn-size reconstruction: each eight-sample block axis keeps n cells
//! (n = 1, 2, 3, 4 or 6), the area means of its T.81 A.3.3 samples.
//!
//! Up to four cells per axis, an integrated cosine basis maps dequantized
//! coefficients straight to the cell means of the unclamped samples, which
//! are rounded and clamped once; every coefficient contributes. Six cells
//! average the clamped, rounded full block instead. Where a block overshoots
//! the sample range, the unclamped means differ from the exact path's means
//! of clamped samples, so this reconstruction is approximate (ADR 0004).
use super::{Idct, ORDER, Sample};

/// cos(kπ/16) for k = 0..=8, written out so that every target evaluates the
/// same basis; the exact path's guarded transform does not depend on it.
const COSINE: [f64; 9] = [
    1.0,
    0.980_785_280_403_230_4,
    0.923_879_532_511_286_7,
    0.831_469_612_302_545_2,
    std::f64::consts::FRAC_1_SQRT_2,
    0.555_570_233_019_602_2,
    0.382_683_432_365_089_8,
    0.195_090_322_016_128_3,
    0.0,
];

/// cos(mπ/16) for any integer m, by symmetry from [`COSINE`].
fn cosine(m: usize) -> f64 {
    let m = m % 32;
    let m = if m > 16 { 32 - m } else { m };
    if m <= 8 { COSINE[m] } else { -COSINE[16 - m] }
}

/// The overlap of sample `x` with cell `j` of `n`, in eighths of the cell:
/// the cell spans [8j/n, 8(j+1)/n) and the sample [x, x+1).
fn overlap(x: usize, j: usize, n: usize) -> u32 {
    let low = (n * x).max(8 * j);
    let high = (n * (x + 1)).min(8 * (j + 1));
    u32::try_from(high.saturating_sub(low)).unwrap_or(0)
}

/// Cells kept per block axis, with the basis or weights that produce them.
pub(in crate::dct) struct Cells {
    count: usize,
    /// For one to four cells: the mean of each frequency's basis function
    /// over each cell, scaled as the T.81 A.3.3 IDCT.
    basis: [[f32; 4]; 8],
}

impl Cells {
    /// `count` must be 1, 2, 3, 4 or 6.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the integrated basis is evaluated in f32 by this approximate path"
    )]
    pub(in crate::dct) fn new(count: usize) -> Self {
        let mut basis = [[0.0; 4]; 8];
        if count <= 4 {
            for (u, row) in basis.iter_mut().enumerate() {
                let scale = if u == 0 { COSINE[4] * 0.5 } else { 0.5 };
                for (j, value) in row.iter_mut().take(count).enumerate() {
                    let mut sum = 0.0;
                    for x in 0..8 {
                        let weight = f64::from(overlap(x, j, count)) / 8.0;
                        sum += weight * cosine((2 * x + 1) * u);
                    }
                    *value = (sum * scale) as f32;
                }
            }
        }
        Self { count, basis }
    }

    /// Reconstructs one block of zigzag coefficients into its `count` rows
    /// of `count` cells in a component plane of `stride` blocks per row.
    /// Coefficients past the slice are zero; it holds at least the DC
    /// coefficient.
    pub(in crate::dct) fn reconstruct(
        &self,
        idct: &Idct,
        quant: &[u16; 64],
        coefficients: &[i32],
        plane: &mut [u8],
        stride: usize,
        index: usize,
    ) -> Option<()> {
        let n = self.count;
        let pitch = stride * n;
        let start = (index / stride * n * pitch).checked_add(index % stride * n)?;
        let rows = plane.get_mut(start..start.checked_add((n - 1) * pitch + n)?)?;
        let mut block = [[0u8; 6]; 6];
        let (&dc, ac) = coefficients.split_first()?;
        if n == 1 || ac.iter().all(|&v| v == 0) {
            // A constant block, or the DC mean of every unclamped sample.
            let value = idct.constant::<u8>(f64::from(dc) * f64::from(quant[0]));
            block = [[value; 6]; 6];
        } else if n == 6 {
            let mut natural = [0.0; 64];
            for ((&coefficient, &q), &position) in coefficients.iter().zip(quant).zip(&ORDER) {
                natural[position] = f64::from(coefficient) * f64::from(q);
            }
            block = six(&idct.block::<u8>(&natural));
        } else {
            let mut natural = [[0.0f32; 8]; 8];
            for ((&coefficient, &q), &position) in coefficients.iter().zip(quant).zip(&ORDER) {
                if coefficient != 0 {
                    natural[position / 8][position % 8] = dequantize(coefficient, q);
                }
            }
            for (out, row) in block.iter_mut().zip(self.means(&natural)) {
                out[..4].copy_from_slice(&row);
            }
        }
        for (row, cells) in rows.chunks_mut(pitch).zip(&block) {
            row.get_mut(..n)?.copy_from_slice(&cells[..n]);
        }
        Some(())
    }

    /// Separable cell means of the unclamped samples, rounded once.
    fn means(&self, natural: &[[f32; 8]; 8]) -> [[u8; 4]; 4] {
        let n = self.count;
        let mut horizontal = [[0.0f32; 4]; 8];
        for (input, output) in natural.iter().zip(&mut horizontal) {
            for (&value, basis) in input.iter().zip(&self.basis) {
                if value == 0.0 {
                    continue;
                }
                for (out, &weight) in output.iter_mut().zip(basis).take(n) {
                    *out += value * weight;
                }
            }
        }
        let mut vertical = [[0.0f32; 4]; 4];
        for (input, basis) in horizontal.iter().zip(&self.basis) {
            if input.iter().all(|&value| value == 0.0) {
                continue;
            }
            for (output, &weight) in vertical.iter_mut().zip(basis).take(n) {
                for (out, &value) in output.iter_mut().zip(input).take(n) {
                    *out += value * weight;
                }
            }
        }
        vertical.map(|row| row.map(|value| u8::from_idct(f64::from(value))))
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "dequantized coefficients are evaluated in f32 by this approximate path"
)]
fn dequantize(coefficient: i32, quant: u16) -> f32 {
    (i64::from(coefficient) * i64::from(quant)) as f32
}

/// Six cells per axis: exact rational means of the reconstructed samples
/// over cells of four thirds of a sample, rounded half up once.
fn six(block: &[[u8; 8]; 8]) -> [[u8; 6]; 6] {
    // In eighths of a cell, each half block [a, b, c, d] of samples gives
    // the cells 6a+2b, 4b+4c and 2c+6d (see `overlap`).
    fn reduce([a, b, c, d, e, f, g, h]: [u32; 8]) -> [u32; 6] {
        [
            6 * a + 2 * b,
            4 * (b + c),
            2 * c + 6 * d,
            6 * e + 2 * f,
            4 * (f + g),
            2 * g + 6 * h,
        ]
    }
    let rows = block.map(|row| reduce(row.map(u32::from)));
    let mut out = [[0u8; 6]; 6];
    for x in 0..6 {
        let column = reduce(std::array::from_fn(|y| rows[y][x]));
        for (row, value) in out.iter_mut().zip(column) {
            row[x] = u8::try_from((value + 32) / 64).unwrap_or(u8::MAX);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{Cells, Idct, ORDER, overlap};

    /// Annex A sample value of one frequency pair at sample (x, y), in f64
    /// from the platform cosine, independent of the written-out table.
    fn basis(u: usize, v: usize, x: usize, y: usize) -> f64 {
        let c = |k: usize| {
            if k == 0 {
                std::f64::consts::FRAC_1_SQRT_2
            } else {
                1.0
            }
        };
        #[expect(clippy::cast_precision_loss, reason = "small indices")]
        let angle = |s: usize, k: usize| ((2 * s + 1) * k) as f64 * std::f64::consts::PI / 16.0;
        0.25 * c(u) * c(v) * angle(x, u).cos() * angle(y, v).cos()
    }

    /// The cell mean of the unclamped samples, from fractional overlaps.
    fn oracle(natural: &[f64; 64], n: usize, i: usize, j: usize) -> f64 {
        let mut sum = 0.0;
        for y in 0..8 {
            for x in 0..8 {
                let weight = f64::from(overlap(x, j, n) * overlap(y, i, n)) / 64.0;
                if weight == 0.0 {
                    continue;
                }
                let mut sample = 0.0;
                for (k, &value) in natural.iter().enumerate() {
                    sample += value * basis(k % 8, k / 8, x, y);
                }
                sum += weight * sample;
            }
        }
        sum + 128.0
    }

    #[test]
    fn overlaps_partition_every_cell_and_sample() {
        for n in [1, 2, 3, 4, 6] {
            for j in 0..n {
                assert_eq!((0..8).map(|x| overlap(x, j, n)).sum::<u32>(), 8);
            }
            for x in 0..8 {
                let total: u32 = (0..n).map(|j| overlap(x, j, n)).sum();
                assert_eq!(total, u32::try_from(n).unwrap());
            }
        }
    }

    #[test]
    fn cell_means_round_the_unclamped_annex_a_means_once() {
        let idct = Idct::new();
        let mut state = 0x5eed_1234u32;
        for n in [1, 2, 3, 4] {
            let cells = Cells::new(n);
            for case in 0..600 {
                let mut coefficients = [0i32; 64];
                if case < 64 {
                    coefficients[case] = 96;
                } else {
                    for value in &mut coefficients {
                        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                        if state >> 29 == 0 || case % 3 == 0 {
                            *value = i32::try_from((state >> 16) % 129).unwrap() - 64;
                        }
                    }
                }
                let quant = [3u16; 64];
                let mut plane = vec![0u8; n * n];
                cells
                    .reconstruct(&idct, &quant, &coefficients, &mut plane, 1, 0)
                    .unwrap();
                let mut natural = [0.0; 64];
                for (&coefficient, &position) in coefficients.iter().zip(&ORDER) {
                    natural[position] = f64::from(coefficient) * 3.0;
                }
                for i in 0..n {
                    for j in 0..n {
                        let expected = oracle(&natural, n, i, j).clamp(0.0, 255.0);
                        let actual = f64::from(plane[i * n + j]);
                        assert!(
                            (actual - expected).abs() <= 0.5 + 1e-3,
                            "n={n} case={case} ({i},{j}): {actual} vs {expected}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn six_cells_average_the_exact_block_with_one_rounding() {
        let idct = Idct::new();
        let cells = Cells::new(6);
        let mut state = 0x0bad_cafeu32;
        for _ in 0..200 {
            let mut coefficients = [0i32; 64];
            for value in coefficients.iter_mut().take(20) {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                *value = i32::try_from((state >> 16) % 257).unwrap() - 128;
            }
            let quant = [5u16; 64];
            let mut natural = [0.0; 64];
            for (&coefficient, &position) in coefficients.iter().zip(&ORDER) {
                natural[position] = f64::from(coefficient) * 5.0;
            }
            let full = idct.block::<u8>(&natural);
            let mut plane = vec![0u8; 36];
            cells
                .reconstruct(&idct, &quant, &coefficients, &mut plane, 1, 0)
                .unwrap();
            for i in 0..6 {
                for j in 0..6 {
                    let mut sum = 0u32;
                    for (y, row) in full.iter().enumerate() {
                        for (x, &sample) in row.iter().enumerate() {
                            sum += overlap(x, j, 6) * overlap(y, i, 6) * u32::from(sample);
                        }
                    }
                    // sum/64 rounded half up.
                    assert_eq!(u32::from(plane[i * 6 + j]), (2 * sum + 64) / 128);
                }
            }
        }
    }

    #[test]
    fn blocks_land_in_their_reduced_plane_cells() {
        let idct = Idct::new();
        for n in [1, 2, 3, 4, 6] {
            let cells = Cells::new(n);
            let (stride, rows) = (3, 2);
            let mut plane = vec![0u8; stride * rows * n * n];
            for index in 0..stride * rows {
                let mut coefficients = [0i32; 64];
                coefficients[0] = i32::try_from(8 * index).unwrap();
                cells
                    .reconstruct(&idct, &[1; 64], &coefficients, &mut plane, stride, index)
                    .unwrap();
            }
            for (offset, &value) in plane.iter().enumerate() {
                let (y, x) = (offset / (stride * n), offset % (stride * n));
                let block = y / n * stride + x / n;
                assert_eq!(usize::from(value), 128 + block);
            }
            assert!(
                cells
                    .reconstruct(&idct, &[1; 64], &[0; 64], &mut plane, stride, stride * rows)
                    .is_none()
            );
        }
    }
}
