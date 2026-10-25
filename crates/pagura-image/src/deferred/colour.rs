//! PDF Table 13 YCbCr conversion of byte components, in the reference's
//! operation order, with f32 lanes and exact binary64 green half ties.

/// Tests a constant byte run with eight-byte word comparisons.
#[must_use]
pub fn uniform(input: &[u8], value: u8) -> bool {
    let pattern = u64::from(value) * 0x0101_0101_0101_0101;
    let (words, tail) = input.as_chunks::<8>();
    words
        .iter()
        .all(|word| u64::from_le_bytes(*word) == pattern)
        && tail.iter().all(|&v| v == value)
}

/// `floor(x)` of `x` clamped to `0..=255`, with NaN as 0: the byte that a
/// saturating float-to-integer conversion of the clamped value gives.
///
/// Each target takes the form its vector unit handles; all give the same
/// byte for every input. Baseline x86-64 has no saturating vector
/// conversion, so there the value is shifted by 2^23, which leaves the
/// nearest integer in the low mantissa bits, and one comparison turns it
/// into the floor. WebAssembly SIMD has neither lane-wise `max` and `min`
/// with Rust's NaN rule nor a conversion to bytes, so there the value
/// converts to `u32` and the integer is clamped. Other targets clamp and
/// convert directly.
#[inline]
#[must_use]
pub fn floor_byte(x: f32) -> u8 {
    if cfg!(any(target_arch = "x86", target_arch = "x86_64")) {
        floor_byte_shifted(x)
    } else if cfg!(target_family = "wasm") {
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

#[inline]
fn quantize(value: f32) -> u8 {
    floor_byte(value)
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
        1 => convert::<1, 16, 48>(luma, cb, cr, output),
        2 => convert::<2, 16, 48>(luma, cb, cr, output),
        4 => convert::<4, 16, 48>(luma, cb, cr, output),
        _ => {}
    }
    true
}
fn convert<const REPEAT: usize, const L: usize, const RGB: usize>(
    luma: &[u8],
    cb: &[u8],
    cr: &[u8],
    output: &mut [u8],
) {
    let wide = luma.len() / L * L;
    for (((ys, target), cbs), crs) in luma[..wide]
        .as_chunks::<L>()
        .0
        .iter()
        .zip(output[..wide * 3].as_chunks_mut::<RGB>().0)
        .zip(cb.chunks(L / REPEAT))
        .zip(cr.chunks(L / REPEAT))
    {
        if uniform(cbs, 128) && uniform(crs, 128) {
            for (pixel, &y) in target.as_chunks_mut::<3>().0.iter_mut().zip(ys) {
                *pixel = [y; 3];
            }
            continue;
        }
        let y = ys.map(f32::from);
        let cb: [f32; L] = std::array::from_fn(|i| f32::from(cbs[i / REPEAT]) - 128.0);
        let cr: [f32; L] = std::array::from_fn(|i| f32::from(crs[i / REPEAT]) - 128.0);
        let r: [u8; L] = std::array::from_fn(|i| quantize(y[i] + cr[i] * 1.402 + 0.5));
        let raw_green: [f32; L] =
            std::array::from_fn(|i| y[i] - cb[i] * 0.344_136 - cr[i] * 0.714_136 + 0.5);
        let mut g = raw_green.map(quantize);
        let ambiguous = raw_green
            .map(|v| (1.0..=255.0).contains(&v) && ((v - nearest_integer(v)).abs() < 0.0001));
        if ambiguous.iter().any(|&v| v) {
            for i in 0..L {
                if ambiguous[i] {
                    g[i] = green(f64::from(y[i]), f64::from(cb[i]), f64::from(cr[i]));
                }
            }
        }
        let b: [u8; L] = std::array::from_fn(|i| quantize(y[i] + cb[i] * 1.772 + 0.5));
        for (i, pixel) in target.as_chunks_mut::<3>().0.iter_mut().enumerate() {
            *pixel = [r[i], g[i], b[i]];
        }
    }
    for (i, pixel) in output[wide * 3..]
        .as_chunks_mut::<3>()
        .0
        .iter_mut()
        .enumerate()
    {
        let x = wide + i;
        let y = f32::from(luma[x]);
        let cb = f32::from(cb[x / REPEAT]) - 128.0;
        let cr = f32::from(cr[x / REPEAT]) - 128.0;
        *pixel = [
            quantize(y + cr * 1.402 + 0.5),
            green(f64::from(y), f64::from(cb), f64::from(cr)),
            quantize(y + cb * 1.772 + 0.5),
        ];
    }
}

/// The integer nearest to `v`, |v| < 2²², or either neighbour of a half.
/// WebAssembly and baseline x86-64 have no rounding instruction, so `round`
/// is a library call there; adding and removing 2²³ rounds in the addition
/// instead.
#[inline]
fn nearest_integer(v: f32) -> f32 {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64", target_family = "wasm"))]
    {
        (v + 8_388_608.0) - 8_388_608.0
    }
    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64", target_family = "wasm")))]
    {
        v.round()
    }
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
    #[expect(clippy::cast_precision_loss, reason = "small exact integers")]
    fn shifted_nearest_integers_mark_the_same_ambiguous_greens() {
        // The x86 and WebAssembly form of `nearest_integer`, checked on every target.
        let shifted = |v: f32| (v + 8_388_608.0) - 8_388_608.0;
        let mut values = Vec::new();
        for k in -2..=258 {
            let k = k as f32;
            for offset in [0.0, 0.00009, 0.0001, 0.00011, 0.25, 0.4999, 0.5, 0.5001] {
                for v in [k + offset, k - offset] {
                    values.extend([v, v.next_up(), v.next_down()]);
                }
            }
        }
        values.extend((0..300 * 1024).map(|step| step as f32 / 1024.0 - 20.0));
        for v in values {
            let ambiguous = |n: f32| (1.0..=255.0).contains(&v) && (v - n).abs() < 0.0001;
            assert_eq!(ambiguous(shifted(v)), ambiguous(v.round()), "{v}");
            assert_eq!(
                ambiguous(super::nearest_integer(v)),
                ambiguous(v.round()),
                "{v}"
            );
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
