//! A scaled f32 inverse cosine transform over eight independent lanes.
//! Both passes use the same sum/difference network, and every lane keeps its
//! multiplication and addition order. Its error against the reference is
//! bounded by the caller's certificate, not by matching operation order.
#![expect(
    clippy::inline_always,
    reason = "fixed lane arithmetic is exposed to the optimizer"
)]
/// One value per lane: a block row or column.
#[derive(Clone, Copy)]
struct Lanes([f32; 8]);
impl std::ops::Add for Lanes {
    type Output = Self;
    #[inline(always)]
    fn add(self, rhs: Self) -> Self {
        Self(std::array::from_fn(|i| self.0[i] + rhs.0[i]))
    }
}
impl std::ops::Sub for Lanes {
    type Output = Self;
    #[inline(always)]
    fn sub(self, rhs: Self) -> Self {
        Self(std::array::from_fn(|i| self.0[i] - rhs.0[i]))
    }
}
impl std::ops::Mul<f32> for Lanes {
    type Output = Self;
    #[inline(always)]
    fn mul(self, rhs: f32) -> Self {
        Self(self.0.map(|v| v * rhs))
    }
}
#[inline(always)]
fn inverse(a: [Lanes; 8]) -> [Lanes; 8] {
    let sum04 = a[0] + a[4];
    let diff04 = a[0] - a[4];
    let sum26 = a[2] + a[6];
    let rot26 = (a[2] - a[6]) * std::f32::consts::SQRT_2 - sum26;
    let even = [sum04 + sum26, diff04 + rot26, diff04 - rot26, sum04 - sum26];
    let sum35 = a[3] + a[5];
    let diff53 = a[5] - a[3];
    let sum17 = a[1] + a[7];
    let diff17 = a[1] - a[7];
    let outer = sum17 + sum35;
    let bridge = (sum17 - sum35) * std::f32::consts::SQRT_2;
    let rotation = (diff53 + diff17) * 1.847_759_1;
    let adjacent = rotation - diff53 * 2.613_126 - outer;
    let inner = bridge - adjacent;
    let center = rotation - diff17 * 1.082_392_2 - inner;
    [
        even[0] + outer,
        even[1] + adjacent,
        even[2] + inner,
        even[3] + center,
        even[3] - center,
        even[2] - inner,
        even[1] - adjacent,
        even[0] - outer,
    ]
}
/// The scaled basis factors of the coefficients' frequencies.
const SCALES: [f32; 8] = [
    0.353_553_38,
    0.490_392_63,
    0.461_939_75,
    0.415_734_8,
    0.353_553_38,
    0.277_785_12,
    0.191_341_71,
    0.097_545_16,
];

/// One pass: the transform of every lane over its eight frequencies.
#[inline(always)]
fn pass(frequencies: [[f32; 8]; 8]) -> [[f32; 8]; 8] {
    let mut scaled = frequencies.map(Lanes);
    for (lanes, scale) in scaled.iter_mut().zip(SCALES) {
        *lanes = *lanes * scale;
    }
    inverse(scaled).map(|lanes| lanes.0)
}

#[inline(always)]
fn transpose(block: [[f32; 8]; 8]) -> [[f32; 8]; 8] {
    std::array::from_fn(|x| block.map(|row| row[x]))
}

/// Unshifted spatial values, in natural order, of a dequantized block given
/// column by column: `columns[u][v]` is the coefficient of vertical frequency
/// `v` and horizontal frequency `u`. The first pass has a lane per row, the
/// second a lane per column.
#[inline(always)]
pub(super) fn spatial(columns: &[[f32; 8]; 8]) -> [[f32; 8]; 8] {
    pass(transpose(pass(*columns)))
}
