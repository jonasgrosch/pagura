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

#[inline]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::manual_clamp,
    reason = "rounded samples are clamped to the byte range"
)]
fn quantize(value: f32) -> u8 {
    (value.max(0.0).min(255.0) as u32).to_le_bytes()[0]
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
        let ambiguous =
            raw_green.map(|v| (1.0..=255.0).contains(&v) && ((v - v.round()).abs() < 0.0001));
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
                    for interval in [false, true] {
                        let mut deficits = vec![[0u16; 4]; 256];
                        assert!(colour.deficits(&ys, interval, &mut deficits));
                        let mut varying = vec![[0u16; 4]; 256];
                        assert!(super::deficits(
                            &ys,
                            &[cb; 256],
                            &[cr; 256],
                            1,
                            interval,
                            &mut varying
                        ));
                        assert_eq!(varying, deficits);
                        for (y, p) in deficits.iter().enumerate() {
                            let high = if interval { (y + 1).min(255) } else { y };
                            for c in 0..3 {
                                assert_eq!(
                                    p[c],
                                    510 - u16::from(actual[y * 3 + c])
                                        - u16::from(actual[high * 3 + c])
                                );
                            }
                            assert_eq!(p[3], 0);
                        }
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
    pub fn new(cb: u8, cr: u8) -> Option<Self> {
        let cb = i32::from(cb) - 128;
        let cr = i32::from(cr) - 128;
        let g = -43_017 * cb - 89_267 * cr;
        if g.rem_euclid(125_000) == 62_500 {
            return None;
        }
        Some(Self {
            offsets: [
                ((175_250 * cr + 62_500).div_euclid(125_000) + 256) as u16,
                ((g + 62_500).div_euclid(125_000) + 256) as u16,
                ((221_500 * cb + 62_500).div_euclid(125_000) + 256) as u16,
            ],
        })
    }
    /// Emits doubled RGB deficits from white, using an optional adjacent luma interval.
    #[must_use]
    pub fn deficits(&self, luma: &[u8], interval: bool, output: &mut [[u16; 4]]) -> bool {
        if luma.len() != output.len() {
            return false;
        }
        let wide = luma.len() / 8 * 8;
        for (ys, out) in luma[..wide]
            .as_chunks::<8>()
            .0
            .iter()
            .zip(output[..wide].as_chunks_mut::<8>().0)
        {
            let low = ys.map(u16::from);
            let high = if interval {
                ys.map(|v| u16::from(v.saturating_add(1)))
            } else {
                low
            };
            let channel = |offset: u16| {
                std::array::from_fn::<_, 8, _>(|i| {
                    510 - (low[i].wrapping_add(offset).saturating_sub(256).min(255)
                        + high[i].wrapping_add(offset).saturating_sub(256).min(255))
                })
            };
            let r = channel(self.offsets[0]);
            let g = channel(self.offsets[1]);
            let b = channel(self.offsets[2]);
            for (i, pixel) in out.iter_mut().enumerate() {
                *pixel = [r[i], g[i], b[i], 0];
            }
        }
        for (&y, out) in luma[wide..].iter().zip(&mut output[wide..]) {
            let low = u16::from(y);
            let high = u16::from(if interval { y.saturating_add(1) } else { y });
            let p = self.offsets.map(|offset| {
                510 - (low.wrapping_add(offset).saturating_sub(256).min(255)
                    + high.wrapping_add(offset).saturating_sub(256).min(255))
            });
            *out = [p[0], p[1], p[2], 0];
        }
        true
    }
}

/// Doubled RGB deficits from white for `luma` with chroma replicated
/// `repeat` times, as [`Constant::deficits`] for each chroma pair. Returns
/// false, with `output` unspecified, for mismatched extents, an unsupported
/// repeat or an exact green half tie.
#[must_use]
pub fn deficits(
    luma: &[u8],
    cb: &[u8],
    cr: &[u8],
    repeat: usize,
    interval: bool,
    output: &mut [[u16; 4]],
) -> bool {
    if !matches!(repeat, 1 | 2 | 4)
        || luma.len() != output.len()
        || cb.len() < luma.len().div_ceil(repeat)
        || cr.len() < luma.len().div_ceil(repeat)
    {
        return false;
    }
    let up = u8::from(interval);
    for ((ys, out), (&cb, &cr)) in luma
        .chunks(repeat)
        .zip(output.chunks_mut(repeat))
        .zip(cb.iter().zip(cr))
    {
        let Some(colour) = Constant::new(cb, cr) else {
            return false;
        };
        for (&y, pixel) in ys.iter().zip(out) {
            let low = u16::from(y);
            let high = u16::from(y.saturating_add(up));
            let p = colour.offsets.map(|offset| {
                510 - (low.wrapping_add(offset).saturating_sub(256).min(255)
                    + high.wrapping_add(offset).saturating_sub(256).min(255))
            });
            *pixel = [p[0], p[1], p[2], 0];
        }
    }
    true
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
