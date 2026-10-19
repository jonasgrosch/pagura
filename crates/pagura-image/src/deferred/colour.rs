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
/// Baseline x86-64 has no saturating vector conversion, so there the value
/// is shifted by 2^23, which leaves the nearest integer in the low mantissa
/// bits, and one comparison turns it into the floor. Other targets convert
/// directly. Both give the same byte for every input.
#[inline]
#[must_use]
#[expect(
    clippy::manual_clamp,
    reason = "max and min map NaN to zero, as the saturating conversion does"
)]
pub fn floor_byte(x: f32) -> u8 {
    let x = x.max(0.0).min(255.0);
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        const SHIFT: f32 = 8_388_608.0;
        let shifted = x + SHIFT;
        let nearest = shifted - SHIFT;
        shifted.to_bits().to_le_bytes()[0].wrapping_sub(u8::from(nearest > x))
    }
    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
    {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the value is clamped to the byte range"
        )]
        let byte = x as u8;
        byte
    }
}

/// The shifted form of [`floor_byte`], tested on every target.
#[cfg(test)]
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
/// Baseline x86-64 has no rounding instruction, so `round` is a library
/// call there; adding and removing 2²³ rounds in the addition instead.
#[inline]
fn nearest_integer(v: f32) -> f32 {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        (v + 8_388_608.0) - 8_388_608.0
    }
    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
    {
        v.round()
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "rounded clamped byte"
)]
fn green(y: f64, cb: f64, cr: f64) -> u8 {
    (y - cb * 0.344_136 - cr * 0.714_136)
        .round()
        .clamp(0.0, 255.0) as u8
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
        for x in special {
            assert_eq!(super::floor_byte(x), reference(x), "{x}");
            assert_eq!(super::floor_byte_shifted(x), reference(x), "{x}");
        }
        for bits in (0..=u32::MAX).step_by(4099) {
            let x = f32::from_bits(bits);
            assert_eq!(super::floor_byte(x), reference(x), "{bits:#x}");
            assert_eq!(super::floor_byte_shifted(x), reference(x), "{bits:#x}");
        }
    }

    #[test]
    #[expect(clippy::cast_precision_loss, reason = "small exact integers")]
    fn shifted_nearest_integers_mark_the_same_ambiguous_greens() {
        // The x86 form of `nearest_integer`, checked on every target.
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
                let mut eight = vec![0; ys.len() * 3];
                assert!(super::ycbcr8(&ys, &[cb; 256], &[cr; 256], 1, &mut eight));
                assert_eq!(actual, eight);
                if let Some(colour) = super::Constant::new(cb, cr) {
                    let mut deficits = vec![[0u16; 4]; 256];
                    assert!(colour.deficits(&ys, &mut deficits));
                    let mut varying = vec![[0u16; 4]; 256];
                    assert!(super::deficits(
                        &ys,
                        &[cb; 256],
                        &[cr; 256],
                        1,
                        &mut varying
                    ));
                    assert_eq!(varying, deficits);
                    for (y, p) in deficits.iter().enumerate() {
                        for c in 0..3 {
                            assert_eq!(p[c], 255 - u16::from(actual[y * 3 + c]));
                        }
                        assert_eq!(p[3], 0);
                    }
                }
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

/// Integer Table 13 conversion for one constant non-tie chroma pair.
#[derive(Debug, Clone, Copy)]
pub struct Constant {
    offsets: [u16; 3],
}
impl Constant {
    /// Returns None at exact green half ties, which retain ordered f64 arithmetic.
    #[must_use]
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "biased chroma offsets are in 0..512"
    )]
    pub fn new(blue: u8, red: u8) -> Option<Self> {
        let cb = i32::from(blue) - 128;
        let cr = i32::from(red) - 128;
        let g = -43_017 * cb - 89_267 * cr;
        if g.rem_euclid(125_000) == 62_500 {
            return None;
        }
        Some(Self {
            offsets: [
                RED[usize::from(red)],
                ((g + 62_500).div_euclid(125_000) + 256) as u16,
                BLUE[usize::from(blue)],
            ],
        })
    }
    /// Emits RGB deficits from white, `255 - channel`, for `luma`.
    #[must_use]
    pub fn deficits(&self, luma: &[u8], output: &mut [[u16; 4]]) -> bool {
        if luma.len() != output.len() {
            return false;
        }
        for (ys, out) in luma.chunks(8).zip(output.chunks_mut(8)) {
            let [r, g, b] = lanes(ys, self.offsets.map(|offset| [offset; 8]));
            for (i, pixel) in out.iter_mut().enumerate() {
                *pixel = [r[i], g[i], b[i], 0];
            }
        }
        true
    }
}

/// Deficits `255 - clamp(Y + offset - 256)` of up to eight samples per
/// channel, from biased Table 13 offsets per sample.
#[expect(
    clippy::inline_always,
    reason = "fixed eight-sample lanes vectorize only when inlined"
)]
#[inline(always)]
fn lanes(luma: &[u8], offsets: [[u16; 8]; 3]) -> [[u16; 8]; 3] {
    let y: [u16; 8] = std::array::from_fn(|i| luma.get(i).map_or(0, |&v| u16::from(v)));
    offsets.map(|offset| {
        std::array::from_fn(|i| 255 - y[i].wrapping_add(offset[i]).saturating_sub(256).min(255))
    })
}

/// RGB deficits from white for `luma` with chroma replicated `repeat`
/// times, as [`Constant::deficits`] for each chroma pair. Returns false,
/// with `output` unspecified, for mismatched extents, an unsupported repeat
/// or an exact green half tie.
#[must_use]
pub fn deficits(luma: &[u8], cb: &[u8], cr: &[u8], repeat: usize, output: &mut [[u16; 4]]) -> bool {
    if !matches!(repeat, 1 | 2 | 4)
        || luma.len() != output.len()
        || cb.len() < luma.len().div_ceil(repeat)
        || cr.len() < luma.len().div_ceil(repeat)
    {
        return false;
    }
    let shift = repeat.trailing_zeros();
    for (group, (ys, out)) in luma.chunks(8).zip(output.chunks_mut(8)).enumerate() {
        // Eight samples span at most eight chroma pairs; repeat divides 8.
        let first = (group * 8) >> shift;
        let mut pairs = [[0u16; 3]; 8];
        for (k, pair) in pairs[..(ys.len() + repeat - 1) >> shift]
            .iter_mut()
            .enumerate()
        {
            let Some(colour) = Constant::new(cb[first + k], cr[first + k]) else {
                return false;
            };
            *pair = colour.offsets;
        }
        let offsets = std::array::from_fn(|c| std::array::from_fn(|i| pairs[i >> shift][c]));
        let [r, g, b] = lanes(ys, offsets);
        for (i, pixel) in out.iter_mut().enumerate() {
            *pixel = [r[i], g[i], b[i], 0];
        }
    }
    true
}

/// Biased Table 13 offsets `round(1.402 (Cr - 128)) + 256` and
/// `round(1.772 (Cb - 128)) + 256`, by chroma byte.
const RED: [u16; 256] = offsets(175_250);
const BLUE: [u16; 256] = offsets(221_500);

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    reason = "biased chroma offsets are in 0..512"
)]
const fn offsets(coefficient: i32) -> [u16; 256] {
    let mut table = [0; 256];
    let mut index = 0;
    while index < 256 {
        let chroma = index as i32 - 128;
        table[index] = ((coefficient * chroma + 62_500).div_euclid(125_000) + 256) as u16;
        index += 1;
    }
    table
}

/// As [`ycbcr`], in groups of eight samples for short runs.
#[must_use]
pub fn ycbcr8(luma: &[u8], cb: &[u8], cr: &[u8], repeat: usize, output: &mut [u8]) -> bool {
    if !matches!(repeat, 1 | 2 | 4)
        || luma.len().checked_mul(3) != Some(output.len())
        || cb.len() < luma.len().div_ceil(repeat)
        || cr.len() < luma.len().div_ceil(repeat)
    {
        return false;
    }
    match repeat {
        1 => convert::<1, 8, 24>(luma, cb, cr, output),
        2 => convert::<2, 8, 24>(luma, cb, cr, output),
        4 => convert::<4, 8, 24>(luma, cb, cr, output),
        _ => {}
    }
    true
}
