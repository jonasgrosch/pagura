//! Portable one-round byte source-over for the explicit low-precision policy.
//! Weighted byte sums are at most 65025; adding the half-rounding bias and
//! its high byte remains below 65536. Wrapping lane arithmetic cannot wrap.

#[inline]
fn divide(value: u16) -> u16 {
    let value = value.wrapping_add(128);
    value.wrapping_add(value >> 8) >> 8
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "convex byte mixtures fit in a byte"
)]
pub(super) fn pixel(target: &mut [u8; 4], source: [u8; 4], alpha: u8) {
    let a = u16::from(alpha);
    for (out, source) in target.iter_mut().zip(source) {
        *out = divide(
            u16::from(source)
                .wrapping_mul(a)
                .wrapping_add(u16::from(*out).wrapping_mul(255 - a)),
        ) as u8;
    }
}

#[inline(never)]
#[expect(
    clippy::cast_possible_truncation,
    reason = "rounded byte coverage and convex byte mixtures fit in a byte"
)]
pub(super) fn row(target: &mut [[u8; 4]], coverage: &[u8], source: [u8; 4], alpha: u8) {
    if alpha == 255 && coverage.iter().all(|&c| c == 255) {
        target.fill(source);
        return;
    }
    let (blocks, tail) = target.as_chunks_mut::<4>();
    let (masks, mask_tail) = coverage.as_chunks::<4>();
    for (block, mask) in blocks.iter_mut().zip(masks) {
        let before: [u16; 16] = std::array::from_fn(|i| u16::from(block[i / 4][i % 4]));
        let opacity: [u16; 16] =
            std::array::from_fn(|i| divide(u16::from(mask[i / 4]) * u16::from(alpha)));
        let color: [u16; 16] = std::array::from_fn(|i| u16::from(source[i % 4]));
        let result: [u8; 16] = std::array::from_fn(|i| {
            divide(
                color[i]
                    .wrapping_mul(opacity[i])
                    .wrapping_add(before[i].wrapping_mul(255 - opacity[i])),
            ) as u8
        });
        for (pixel, bytes) in block.iter_mut().zip(result.as_chunks::<4>().0) {
            *pixel = *bytes;
        }
    }
    for (out, &coverage) in tail.iter_mut().zip(mask_tail) {
        pixel(
            out,
            source,
            divide(u16::from(coverage) * u16::from(alpha)) as u8,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn portable_lanes_round_every_byte_mixture_once() {
        for alpha in 0..=255u8 {
            for source in 0..=255u8 {
                let mut pixels: Vec<_> = (0..=255u8).map(|v| [v; 4]).collect();
                row(&mut pixels, &[alpha; 256], [source; 4], 255);
                for (background, result) in pixels.iter().enumerate() {
                    let expected = (u32::from(source) * u32::from(alpha)
                        + u32::try_from(background).unwrap() * (255 - u32::from(alpha))
                        + 127)
                        / 255;
                    assert_eq!(result, &[u8::try_from(expected).unwrap(); 4]);
                }
            }
        }
    }
}
