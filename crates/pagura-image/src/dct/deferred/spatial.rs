//! A scaled f32 inverse cosine transform over four independent lanes.
//! Both passes use the same sum/difference network, and every lane keeps its
//! multiplication and addition order. Its error against the reference is
//! bounded by the caller's certificate, not by matching operation order.
#![expect(
    clippy::inline_always,
    reason = "fixed lane arithmetic is exposed to the optimizer"
)]
#[derive(Clone, Copy)]
struct Four([f32; 4]);
impl Four {
    #[inline(always)]
    fn load(input: &[f32]) -> Self {
        Self(std::array::from_fn(|i| input[i]))
    }
    #[inline(always)]
    fn store_slice(self, output: &mut [f32]) {
        output[..4].copy_from_slice(&self.0);
    }
}
impl std::ops::Add for Four {
    type Output = Self;
    #[inline(always)]
    fn add(self, rhs: Self) -> Self {
        Self(std::array::from_fn(|i| self.0[i] + rhs.0[i]))
    }
}
impl std::ops::Sub for Four {
    type Output = Self;
    #[inline(always)]
    fn sub(self, rhs: Self) -> Self {
        Self(std::array::from_fn(|i| self.0[i] - rhs.0[i]))
    }
}
impl std::ops::Mul<f32> for Four {
    type Output = Self;
    #[inline(always)]
    fn mul(self, rhs: f32) -> Self {
        Self(self.0.map(|v| v * rhs))
    }
}
#[inline(always)]
fn inverse(a: [Four; 8]) -> [Four; 8] {
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
#[inline(always)]
fn transpose(a: [Four; 4]) -> [Four; 4] {
    std::array::from_fn(|i| Four(std::array::from_fn(|j| a[j].0[i])))
}
/// Unshifted spatial values of a dequantized block in natural order.
#[inline(always)]
pub(super) fn spatial(input: &[f32; 64]) -> [[f32; 8]; 8] {
    let scales = [
        0.353_553_38,
        0.490_392_63,
        0.461_939_75,
        0.415_734_8,
        0.353_553_38,
        0.277_785_12,
        0.191_341_71,
        0.097_545_16,
    ];
    let mut horizontal = [[0.0; 8]; 8];
    for group in [0, 4] {
        let lo = transpose(std::array::from_fn(|i| {
            Four::load(&input[(group + i) * 8..(group + i) * 8 + 4])
        }));
        let hi = transpose(std::array::from_fn(|i| {
            Four::load(&input[(group + i) * 8 + 4..(group + i) * 8 + 8])
        }));
        let values = inverse(std::array::from_fn(|i| {
            if i < 4 {
                lo[i] * scales[i]
            } else {
                hi[i - 4] * scales[i]
            }
        }));
        for start in [0, 4] {
            for (i, row) in transpose(std::array::from_fn(|i| values[start + i]))
                .into_iter()
                .enumerate()
            {
                row.store_slice(&mut horizontal[group + i][start..start + 4]);
            }
        }
    }
    let mut output = [[0.0; 8]; 8];
    for x in [0, 4] {
        let values = inverse(std::array::from_fn(|v| {
            Four::load(&horizontal[v][x..x + 4]) * scales[v]
        }));
        for (row, value) in output.iter_mut().zip(values) {
            row[x..x + 4].copy_from_slice(&value.0);
        }
    }
    output
}
