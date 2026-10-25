//! PDF Table 13 YCbCr conversion of byte components in the byte domain:
//! integer luma plus a chroma pair's rounded offset per channel, with the
//! reference's binary64 arithmetic at an exact green half tie.

/// `floor(x)` of `x` clamped to `0..=255`, with NaN as 0: the byte that a
/// saturating float-to-integer conversion of the clamped value gives.
///
/// Each target takes the form its vector unit handles; all give the same
/// byte for every input. Baseline x86-64 has no saturating vector
/// conversion, so there the value is shifted by 2^23, which leaves the
/// nearest integer in the low mantissa bits, and one comparison turns it
/// into the floor. WebAssembly SIMD has neither lane-wise `max` and `min`
/// with Rust's NaN rule nor a conversion to bytes, and AArch64's conversion
/// to `u32` saturates by itself, so there the value converts to `u32` and
/// the integer is clamped. Other targets clamp and convert directly.
#[inline]
#[must_use]
pub fn floor_byte(x: f32) -> u8 {
    if cfg!(any(target_arch = "x86", target_arch = "x86_64")) {
        floor_byte_shifted(x)
    } else if cfg!(any(target_family = "wasm", target_arch = "aarch64")) {
        floor_byte_widened(x)
    } else {
        floor_byte_clamped(x)
    }
}

/// [`floor_byte`] by a shift of 2^23.
#[inline]
#[expect(
    clippy::manual_clamp,
    reason = "max and min map NaN to zero, as the saturating conversion does"
)]
fn floor_byte_shifted(x: f32) -> u8 {
    const SHIFT: f32 = 8_388_608.0;
    let x = x.max(0.0).min(255.0);
    let shifted = x + SHIFT;
    let nearest = shifted - SHIFT;
    shifted.to_bits().to_le_bytes()[0].wrapping_sub(u8::from(nearest > x))
}

/// [`floor_byte`] by a saturating conversion to `u32`, clamped there.
#[inline]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the saturating conversion and the minimum are the clamp"
)]
fn floor_byte_widened(x: f32) -> u8 {
    (x as u32).min(255) as u8
}

/// [`floor_byte`] by a clamp and a saturating conversion.
#[inline]
#[expect(
    clippy::manual_clamp,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "max and min map NaN to zero, as the saturating conversion does"
)]
fn floor_byte_clamped(x: f32) -> u8 {
    x.max(0.0).min(255.0) as u8
}

/// Converts native luma and chroma replicated `repeat` times to interleaved
/// RGB, exactly as Table 13 rounds each byte triple. Returns false for
/// mismatched extents or an unsupported repeat.
#[must_use]
pub fn ycbcr(luma: &[u8], cb: &[u8], cr: &[u8], repeat: usize, output: &mut [u8]) -> bool {
    if !matches!(repeat, 1 | 2 | 4) || luma.len().checked_mul(3) != Some(output.len()) {
        return false;
    }
    let chroma = luma.len().div_ceil(repeat);
    if cb.len() < chroma || cr.len() < chroma {
        return false;
    }
    match repeat {
        1 => convert::<1>(luma, cb, cr, output),
        2 => convert::<2>(luma, cb, cr, output),
        4 => convert::<4>(luma, cb, cr, output),
        _ => {}
    }
    true
}

/// Chroma pairs converted together: fixed lane arrays the compiler vectorizes.
const LANES: usize = 16;

/// Converts every group of `LANES` chroma pairs with the `REPEAT * LANES`
/// luma samples over them; the remaining samples convert as one group,
/// padded.
fn convert<const REPEAT: usize>(luma: &[u8], cb: &[u8], cr: &[u8], output: &mut [u8]) {
    let step = LANES * REPEAT;
    let groups = luma
        .chunks_exact(step)
        .zip(output.chunks_exact_mut(3 * step))
        .zip(
            cb.as_chunks::<LANES>()
                .0
                .iter()
                .zip(cr.as_chunks::<LANES>().0),
        );
    for ((luma, output), (cb, cr)) in groups {
        group::<REPEAT>(luma, (cb, cr), output);
    }
    let done = luma.len() / step * step;
    let rest = luma.len() - done;
    if rest > 0 {
        let (mut padded, mut blue, mut red) = ([0; 4 * LANES], [0; LANES], [0; LANES]);
        let chroma = rest.div_ceil(REPEAT);
        padded[..rest].copy_from_slice(&luma[done..]);
        blue[..chroma].copy_from_slice(&cb[done / REPEAT..][..chroma]);
        red[..chroma].copy_from_slice(&cr[done / REPEAT..][..chroma]);
        let mut pixels = [0; 12 * LANES];
        group::<REPEAT>(&padded[..step], (&blue, &red), &mut pixels[..3 * step]);
        output[3 * done..].copy_from_slice(&pixels[..3 * rest]);
    }
}

/// Converts `LANES` chroma pairs with the `REPEAT * LANES` luma samples
/// over them.
#[inline]
fn group<const REPEAT: usize>(
    luma: &[u8],
    (cb, cr): (&[u8; LANES], &[u8; LANES]),
    output: &mut [u8],
) {
    let (offsets, tied) = offsets(cb, cr);
    // Each pair's offsets over its `REPEAT` luma samples.
    let mut wide = [[[0; REPEAT]; LANES]; 3];
    for (wide, offsets) in wide.iter_mut().zip(&offsets) {
        for (run, &offset) in wide.iter_mut().zip(offsets) {
            *run = [offset; REPEAT];
        }
    }
    let [red, green, blue] = wide.each_ref().map(|channel| channel.as_flattened());
    let pixels = output.as_chunks_mut::<3>().0;
    for (((pixel, &y), (&red, &green)), &blue) in pixels
        .iter_mut()
        .zip(luma)
        .zip(red.iter().zip(green))
        .zip(blue)
    {
        *pixel = [add(y, red), add(y, green), add(y, blue)];
    }
    if tied {
        for (x, (pixel, &y)) in pixels.iter_mut().zip(luma).enumerate() {
            pixel[1] = rgb(y, cb[x / REPEAT], cr[x / REPEAT])[1];
        }
    }
}

/// Table 13's rounded red, green and blue offsets of each chroma pair, and
/// whether any pair sits at an exact green half tie.
#[inline]
fn offsets(cb: &[u8; LANES], cr: &[u8; LANES]) -> ([[i16; LANES]; 3], bool) {
    let mut offsets = [[0; LANES]; 3];
    let mut tied = false;
    let [red, green, blue] = &mut offsets;
    for ((((&cb, &cr), red), green), blue) in cb
        .iter()
        .zip(cr)
        .zip(red.iter_mut())
        .zip(green.iter_mut())
        .zip(blue.iter_mut())
    {
        let tie;
        (*red, *green, *blue, tie) = offset(cb, cr);
        tied |= tie;
    }
    (offsets, tied)
}

/// Table 13's rounded red, green and blue offsets of a chroma pair, each
/// `c · coefficient + 0.5` floored for the decimal coefficients, and
/// whether green sits at a half tie, where the reference's two binary64
/// subtractions decide. Luma adds after rounding. Every numerator lies
/// within ±2.27e8, so the biased quotient is positive and the wrapping
/// operations are exact; free of overflow checks, they vectorize.
#[inline]
#[expect(
    clippy::cast_possible_truncation,
    reason = "Table 13 offsets lie within -227..=227"
)]
pub(crate) fn offset(cb: u8, cr: u8) -> (i16, i16, i16, bool) {
    let (cb, cr) = (
        i32::from(cb).wrapping_sub(128),
        i32::from(cr).wrapping_sub(128),
    );
    let green = (-344_136i32)
        .wrapping_mul(cb)
        .wrapping_sub(714_136i32.wrapping_mul(cr));
    let rounded_green = rounded(green);
    let tie = green == rounded_green.wrapping_mul(1_000_000).wrapping_sub(500_000);
    (
        rounded(1_402_000i32.wrapping_mul(cr)) as i16,
        rounded_green as i16,
        rounded(1_772_000i32.wrapping_mul(cb)) as i16,
        tie,
    )
}

/// `numerator / 10⁶ + 0.5` floored, for |numerator| < 2.56e8.
#[inline]
#[expect(
    clippy::cast_sign_loss,
    reason = "the biased numerator is positive and below 2³¹"
)]
fn rounded(numerator: i32) -> i32 {
    (numerator.wrapping_add(256_500_000) as u32 / 1_000_000)
        .cast_signed()
        .wrapping_sub(256)
}

/// A clamped channel. The sum lies within -227..=482, so the wrapping
/// addition is exact and, free of overflow checks, vectorizes.
#[inline]
#[expect(
    clippy::cast_sign_loss,
    reason = "the sum is clamped to the byte range"
)]
fn add(luma: u8, offset: i16) -> u8 {
    i16::from(luma).wrapping_add(offset).clamp(0, 255) as u8
}

/// One pixel's Table 13 conversion.
fn rgb(y: u8, cb: u8, cr: u8) -> [u8; 3] {
    let (red, green, blue, tie) = offset(cb, cr);
    let mut pixel = [red, green, blue].map(|offset| add(y, offset));
    if tie {
        pixel[1] = exact_green(y, cb, cr);
    }
    pixel
}

/// Green at a half tie, in the reference's binary64 operation order.
fn exact_green(y: u8, cb: u8, cr: u8) -> u8 {
    green(f64::from(y), f64::from(cb) - 128.0, f64::from(cr) - 128.0)
}

fn green(y: f64, cb: f64, cr: f64) -> u8 {
    pagura_core::convert::round_u8(y - cb * 0.344_136 - cr * 0.714_136)
}
#[cfg(test)]
mod tests {
    #[test]
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the reference saturating conversion"
    )]
    fn floor_bytes_equal_saturating_conversions() {
        let reference = |x: f32| x.clamp(0.0, 255.0) as u8;
        let mut special = vec![
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            -0.0,
            0.0,
            f32::MAX,
        ];
        for k in -2..=258 {
            let k = k as f32;
            for x in [k, k - 0.5, k + 0.5] {
                special.extend([x, x.next_up(), x.next_down()]);
            }
        }
        let forms = [
            super::floor_byte,
            super::floor_byte_shifted,
            super::floor_byte_widened,
            super::floor_byte_clamped,
        ];
        for x in special {
            for form in forms {
                assert_eq!(form(x), reference(x), "{x}");
            }
        }
        for bits in (0..=u32::MAX).step_by(4099) {
            let x = f32::from_bits(bits);
            for form in forms {
                assert_eq!(form(x), reference(x), "{bits:#x}");
            }
        }
    }

    #[test]
    fn rows_of_every_repeat_and_length_match_each_pixel() {
        // Chroma pairs at green half ties, placed among pseudo-random ones.
        let ties: Vec<(u8, u8)> = (0..=255u8)
            .flat_map(|cb| (0..=255u8).map(move |cr| (cb, cr)))
            .filter(|&(cb, cr)| super::offset(cb, cr).3)
            .collect();
        let mut state = 0x2545_f491u32;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state.to_le_bytes()[0]
        };
        for repeat in [1, 2, 4] {
            for width in 1..=150usize {
                let chroma = width.div_ceil(repeat);
                let (mut cb, mut cr): (Vec<u8>, Vec<u8>) =
                    (0..chroma).map(|_| (next(), next())).unzip();
                (cb[width % chroma], cr[width % chroma]) = ties[width % ties.len()];
                let luma: Vec<u8> = (0..width).map(|_| next()).collect();
                let mut output = vec![0; 3 * width];
                assert!(super::ycbcr(&luma, &cb, &cr, repeat, &mut output));
                for (x, (pixel, &y)) in output.as_chunks::<3>().0.iter().zip(&luma).enumerate() {
                    let expected = super::rgb(y, cb[x / repeat], cr[x / repeat]);
                    assert_eq!(*pixel, expected, "repeat {repeat} width {width} column {x}");
                }
            }
        }
    }

    #[test]
    fn every_byte_triple_matches_independent_table_13_arithmetic() {
        let ys: Vec<u8> = (0..=255).collect();
        let mut actual = vec![0; 256 * 3];
        for cb in 0..=255u8 {
            for cr in 0..=255u8 {
                assert!(super::ycbcr(&ys, &[cb; 256], &[cr; 256], 1, &mut actual));
                let cb0 = i32::from(cb) - 128;
                let cr0 = i32::from(cr) - 128;
                let tie = (-43_017 * cb0 - 89_267 * cr0).rem_euclid(125_000) == 62_500;
                for pair in actual.as_chunks::<3>().0.windows(2) {
                    for (c, (&low, &high)) in pair[0].iter().zip(&pair[1]).enumerate() {
                        if c != 1 || !tie {
                            assert!(low.abs_diff(high) <= 1, "cb={cb},cr={cr},channel={c}");
                        }
                    }
                }
                for (y, p) in actual.as_chunks::<3>().0.iter().enumerate() {
                    let y = f64::from(u8::try_from(y).unwrap());
                    let cb = f64::from(cb) - 128.0;
                    let cr = f64::from(cr) - 128.0;
                    for (&a, v) in p.iter().zip([
                        y + 1.402 * cr,
                        y - 0.344_136 * cb - 0.714_136 * cr,
                        y + 1.772 * cb,
                    ]) {
                        assert_eq!(f64::from(a), v.round().clamp(0.0, 255.0));
                    }
                }
            }
        }
    }
}
