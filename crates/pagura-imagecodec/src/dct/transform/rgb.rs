//! Exact byte-domain PDF Table 13 conversion using its decimal coefficients.
//! Integer luminance can be added after rounding a chroma contribution, except
//! at a green half tie where the reference's two f64 subtractions matter.
use super::sample;

const fn offsets(coefficient: i32) -> [i32; 256] {
    let mut result = [0; 256];
    let mut index = 0u8;
    loop {
        result[index as usize] =
            (coefficient * (index as i32 - 128) + 500_000).div_euclid(1_000_000);
        if index == 255 {
            break;
        }
        index += 1;
    }
    result
}
const RED: [i32; 256] = offsets(1_402_000);
const BLUE: [i32; 256] = offsets(1_772_000);

pub(super) struct Rgb {
    offsets: [i32; 3],
    cb: i32,
    cr: i32,
    green_tie: bool,
}
impl Rgb {
    #[inline]
    #[expect(
        clippy::cast_sign_loss,
        clippy::cast_possible_wrap,
        reason = "biased green numerator is in 122..393 million, quotient in 122..393"
    )]
    pub(super) fn new(cb: u8, cr: u8) -> Self {
        let (red, blue) = (RED[usize::from(cr)], BLUE[usize::from(cb)]);
        let (cb, cr) = (i32::from(cb) - 128, i32::from(cr) - 128);
        let green = -344_136 * cb - 714_136 * cr;
        let offsets = [
            red,
            ((green + 256_500_000) as u32 / 1_000_000) as i32 - 256,
            blue,
        ];
        Self {
            offsets,
            cb,
            cr,
            green_tie: green == offsets[1] * 1_000_000 - 500_000,
        }
    }

    #[expect(
        clippy::cast_sign_loss,
        reason = "channel sums are clamped to bytes before casting"
    )]
    #[inline]
    pub(super) fn pixel(&self, y: u8) -> [u8; 3] {
        let mut rgb = self
            .offsets
            .map(|offset| (i32::from(y) + offset).clamp(0, 255) as u8);
        if self.green_tie {
            rgb[1] = sample(
                f64::from(y) - 0.344_136 * f64::from(self.cb) - 0.714_136 * f64::from(self.cr),
            );
        }
        rgb
    }
}

#[cfg(test)]
mod tests {
    use super::Rgb;

    #[test]
    fn every_byte_triplet_keeps_table_13_rounding() {
        let mut green_ties = 0;
        for cb in 0..=255u8 {
            for cr in 0..=255u8 {
                let rgb = Rgb::new(cb, cr);
                green_ties += usize::from(rgb.green_tie);
                for y in 0..=255u8 {
                    let exact = super::super::ycbcr(
                        f64::from(y),
                        f64::from(cb) - 128.0,
                        f64::from(cr) - 128.0,
                    )
                    .map(super::super::sample);
                    assert_eq!(rgb.pixel(y), exact, "{y} {cb} {cr}");
                }
            }
        }
        eprintln!(
            "exhaustive RGB: 16777216 triplets, {green_ties} chroma pairs require half-tie evaluation"
        );
    }
}
