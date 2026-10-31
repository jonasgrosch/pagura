//! Normal source-over into narrow RGBA storage (ADR 0004 §5–§6): N16 rounds
//! once per store of an opaque colour at a coverage byte; a dynamic store
//! (any other opacity, or a child's result) is evaluated in f64 and rounded
//! once to the buffer's format; F32 mark stores evaluate `x += a·(t − x)`.
//! Colours are premultiplied with the background embedded.

use pagura_core::convert::{round_f32_u8, round_u8, round_u16};

use super::Colors;

/// `floor(x / 255)` for `x ≤ 255 · 65535 + 127` without a division:
/// `y = 257 x`, then `(y + (y >> 16) + 1) >> 16`. Every intermediate stays
/// below 2^32 (checked exhaustively in the tests).
#[inline]
pub(crate) fn div255(x: u32) -> u32 {
    let y = (x << 8).wrapping_add(x);
    y.wrapping_add(y >> 16).wrapping_add(1) >> 16
}

/// The nearest byte of an N16 word, `round(w / 257)` (never a tie):
/// `n = w + 128`, then `(n − (n >> 8)) >> 8`, in u16 lanes: `n` saturates
/// at 65535 only for words above 65407, whose byte is 255 either way
/// (checked exhaustively in the tests).
#[inline]
pub(crate) fn word_byte(word: u16) -> u8 {
    let n = word.saturating_add(128);
    (n.wrapping_sub(n >> 8) >> 8) as u8
}

/// A row of N16 pixels as output bytes, lane by lane. Kept out of line so
/// that the compiler sees two unaliased slices and vectorizes the loop.
#[inline(never)]
pub(crate) fn n16_bytes(out: &mut [[u8; 4]], words: &[[u16; 4]]) {
    for (byte, &word) in out.as_flattened_mut().iter_mut().zip(words.as_flattened()) {
        *byte = word_byte(word);
    }
}

/// A row of F32 levels as output bytes, rounded half away from zero.
#[inline(never)]
pub(crate) fn f32_bytes(out: &mut [[u8; 4]], levels: &[[f32; 4]]) {
    for (byte, &level) in out.as_flattened_mut().iter_mut().zip(levels.as_flattened()) {
        *byte = round_f32_u8(level);
    }
}

/// Stores an opaque colour, already encoded as N16 `words`, at `coverage`
/// (`None` covers every pixel): `floor((c·s + (255 − c)·u + 127) / 255)`
/// with one integer rounding per store. The formula leaves a pixel as it is
/// at coverage 0 and stores the source at 255, so the loop has no branches
/// and no division, and vectorizes on u32 lanes.
pub(crate) fn store_n16(pixels: &mut [[u16; 4]], words: [u16; 4], coverage: Option<&[u8]>) {
    let Some(coverage) = coverage else {
        pixels.fill(words);
        return;
    };
    let s = words.map(u32::from);
    for (pixel, &c) in pixels.iter_mut().zip(coverage) {
        // One pixel's four channels as four u32 lanes.
        let c = u32::from(c);
        let r = 255_u32.wrapping_sub(c);
        let b = [
            u32::from(pixel[0]),
            u32::from(pixel[1]),
            u32::from(pixel[2]),
            u32::from(pixel[3]),
        ];
        *pixel = [
            mix(s[0], b[0], (c, r)),
            mix(s[1], b[1], (c, r)),
            mix(s[2], b[2], (c, r)),
            mix(s[3], b[3], (c, r)),
        ];
    }
}

/// [`store_n16`] at `coverage`, eight pixels at a time: eight at zero stay
/// as they are and eight at 255 take the words, so only mixed stretches
/// evaluate stores. Returns the pixels from the first to past the last one
/// with coverage, if any. `pixels` and `coverage` have equal lengths.
#[inline]
pub(crate) fn store_n16_runs(
    pixels: &mut [[u16; 4]],
    words: [u16; 4],
    coverage: &[u8],
) -> Option<(usize, usize)> {
    let mut covered: Option<(usize, usize)> = None;
    let mut cover = |lo: usize, hi: usize| {
        covered = Some(covered.map_or((lo, hi), |(first, _)| (first, hi)));
    };
    let (blocks, pixel_tail) = pixels.as_chunks_mut::<8>();
    let (bytes, byte_tail) = coverage.as_chunks::<8>();
    for (k, (block, eight)) in blocks.iter_mut().zip(bytes).enumerate() {
        let word = u64::from_le_bytes(*eight);
        if word == 0 {
            continue;
        }
        if word == u64::MAX {
            *block = [words; 8];
        } else {
            store_n16(block, words, Some(eight));
        }
        // Little-endian: the first pixel is the lowest byte.
        let at = k * 8;
        let lo = at + (word.trailing_zeros() / 8) as usize;
        let hi = at + 8 - (word.leading_zeros() / 8) as usize;
        cover(lo, hi);
    }
    let at = blocks.len() * 8;
    if let Some(first) = byte_tail.iter().position(|&c| c != 0) {
        let last = byte_tail.iter().rposition(|&c| c != 0).unwrap_or(first);
        if let (Some(pixels), Some(coverage)) = (
            pixel_tail.get_mut(first..=last),
            byte_tail.get(first..=last),
        ) {
            store_n16(pixels, words, Some(coverage));
        }
        cover(at + first, at + last + 1);
    }
    covered
}

/// [`store_n16`] with one coverage byte `c` for every pixel.
pub(crate) fn store_n16_uniform(pixels: &mut [[u16; 4]], words: [u16; 4], c: u8) {
    let s = words.map(u32::from);
    let c = u32::from(c);
    let r = 255_u32.wrapping_sub(c);
    for pixel in pixels {
        let b = pixel.map(u32::from);
        *pixel = [
            mix(s[0], b[0], (c, r)),
            mix(s[1], b[1], (c, r)),
            mix(s[2], b[2], (c, r)),
            mix(s[3], b[3], (c, r)),
        ];
    }
}

/// One N16 store; the sum is at most 255 · 65535 + 127.
#[inline]
#[expect(
    clippy::cast_possible_truncation,
    reason = "a convex combination of two words stays within 0..65535"
)]
fn mix(source: u32, backdrop: u32, (c, rest): (u32, u32)) -> u16 {
    div255(
        source
            .wrapping_mul(c)
            .wrapping_add(backdrop.wrapping_mul(rest))
            .wrapping_add(127),
    ) as u16
}

/// The N16 word of a unit value: `257 × L` for its level `L = 255·v`,
/// rounded, moved to the adjacent word where needed so that the final
/// nearest byte is `round(L)` (no double rounding at half levels).
pub(crate) fn word(value: f64) -> u16 {
    let level = value.clamp(0.0, 1.0) * 255.0;
    let word = round_u16(level * 257.0);
    let byte = (u32::from(word) + 128) / 257;
    let nearest = u32::from(round_u8(level));
    match byte.cmp(&nearest) {
        std::cmp::Ordering::Greater => word - 1,
        std::cmp::Ordering::Less => word + 1,
        std::cmp::Ordering::Equal => word,
    }
}

/// A dynamic store into N16 words: `(1 − a)·x + a·t` for the target
/// `t = 65535·c` evaluated in f64 and rounded once to the nearest word.
pub(crate) fn dynamic_n16(pixels: &mut [[u16; 4]], colors: &Colors<'_>, alpha: &[f64]) {
    if let Colors::Strided { values, stride: 3 } = *colors {
        // At full opacity `(1 − a)·x + a·t` is `t`.
        let one = |pixel: &mut [u16; 4], color: &[f64], a: f64| {
            mix_dynamic(pixel, targets(color, 65535.0), a, round_u16, f64::from);
        };
        by_runs((pixels, values, alpha), overwrite_n16, one);
        return;
    }
    dynamic(pixels, colors, alpha, 65535.0, round_u16, f64::from);
}

/// Stores of colours at `alpha` over `pixels`, a value per colour lane of
/// a pixel (all but its last, alpha): each run at full opacity by `opaque`,
/// every other pixel by `one`.
fn by_runs<T, const N: usize>(
    (pixels, values, alpha): (&mut [[T; N]], &[f64], &[f64]),
    opaque: impl Fn(&mut [[T; N]], &[f64]),
    one: impl Fn(&mut [T; N], &[f64], f64),
) {
    let c = N - 1;
    let len = pixels.len().min(alpha.len()).min(values.len() / c);
    if full(&alpha[..len]) {
        opaque(&mut pixels[..len], &values[..len * c]);
        return;
    }
    let mut k = 0;
    while k < len {
        let run = alpha[k..len].iter().take_while(|&&a| a == 1.0).count();
        opaque(&mut pixels[k..k + run], &values[k * c..(k + run) * c]);
        k += run;
        let rest = alpha[k..len].iter().take_while(|&&a| a != 1.0).count();
        let colors = values[k * c..(k + rest) * c].chunks_exact(c);
        for ((pixel, color), &a) in pixels[k..k + rest].iter_mut().zip(colors).zip(&alpha[k..]) {
            one(pixel, color, a);
        }
        k += rest;
    }
}

/// The word a store at full opacity writes for colour value `value`:
/// `65535·value` clamped and rounded. The rounding conversion saturates and
/// takes a NaN to zero, which is also its word, so it is the clamp.
#[inline]
pub(crate) fn opaque_word(value: f64) -> u16 {
    round_u16(value * 65535.0)
}

/// The words of opaque colours, three values a pixel, over `pixels`: a
/// store at full opacity, the word of each target `65535·c`, clamped by
/// [`opaque_word`]'s saturating conversion. Eight pixels at a time, their 24
/// values convert as one run, which vectorizes.
pub(crate) fn overwrite_n16(pixels: &mut [[u16; 4]], values: &[f64]) {
    let word = opaque_word;
    let (blocks, last) = pixels.as_chunks_mut::<8>();
    let (colors, rest) = values.as_chunks::<24>();
    for (block, color) in blocks.iter_mut().zip(colors) {
        let mut words = [0u16; 24];
        for (word_value, &value) in words.iter_mut().zip(color) {
            *word_value = word(value);
        }
        for (pixel, words) in block.iter_mut().zip(words.as_chunks::<3>().0) {
            *pixel = [words[0], words[1], words[2], u16::MAX];
        }
    }
    for (pixel, color) in last.iter_mut().zip(rest.as_chunks::<3>().0) {
        *pixel = [word(color[0]), word(color[1]), word(color[2]), u16::MAX];
    }
}

/// Whether every opacity of `alpha` is 1.
#[expect(
    clippy::needless_bitwise_bool,
    reason = "without a branch per value the test vectorizes"
)]
pub(crate) fn full(alpha: &[f64]) -> bool {
    alpha.iter().fold(true, |all, &a| all & (a == 1.0))
}

/// A dynamic store into F32 levels: `(1 − a)·x + a·t` for the target
/// `t = 255·c` evaluated in f64 and rounded once to f32.
pub(crate) fn dynamic_f32<const N: usize>(
    pixels: &mut [[f32; N]],
    colors: &Colors<'_>,
    alpha: &[f64],
) {
    if let Colors::Strided { values, stride } = *colors
        && stride == N - 1
    {
        let one = |pixel: &mut [f32; N], color: &[f64], a: f64| {
            mix_dynamic(pixel, targets(color, 255.0), a, single, f64::from);
        };
        by_runs((pixels, values, alpha), overwrite_dynamic_f32, one);
        return;
    }
    dynamic(pixels, colors, alpha, 255.0, single, f64::from);
}

/// Dynamic F32 stores of opaque colours, a value per colour lane, at full
/// opacity: `(1 − a)·x + a·t` is `0·x + t`, rounded once.
fn overwrite_dynamic_f32<const N: usize>(pixels: &mut [[f32; N]], values: &[f64]) {
    for (pixel, color) in pixels.iter_mut().zip(values.chunks_exact(N - 1)) {
        for (out, target) in pixel.iter_mut().zip(targets::<N>(color, 255.0)) {
            *out = single(0.0 * f64::from(*out) + target);
        }
    }
}

fn dynamic<T: Copy, const N: usize>(
    pixels: &mut [[T; N]],
    colors: &Colors<'_>,
    alpha: &[f64],
    scale: f64,
    round: impl Fn(f64) -> T,
    widen: impl Fn(T) -> f64,
) {
    let store = |pixel: &mut [T; N], targets: [f64; N], a: f64| {
        mix_dynamic(pixel, targets, a, &round, &widen);
    };
    match *colors {
        Colors::Constant(color) => {
            let targets = targets::<N>(color, scale);
            for (pixel, &a) in pixels.iter_mut().zip(alpha) {
                store(pixel, targets, a);
            }
        }
        Colors::Strided { values, stride } => {
            for ((pixel, color), &a) in pixels
                .iter_mut()
                .zip(values.chunks(stride.max(1)))
                .zip(alpha)
            {
                store(pixel, targets(color, scale), a);
            }
        }
    }
}

/// The targets of a dynamic store of `color`: its clamped values and full
/// alpha on `0 .. scale`.
pub(crate) fn targets<const N: usize>(color: &[f64], scale: f64) -> [f64; N] {
    let mut targets = [scale; N];
    for (i, target) in targets.iter_mut().take(N - 1).enumerate() {
        *target = color.get(i).copied().unwrap_or(0.0).clamp(0.0, 1.0) * scale;
    }
    targets
}

/// One dynamic store, `(1 − a)·x + a·t` rounded once; none at `a = 0`.
#[inline]
fn mix_dynamic<T: Copy, const N: usize>(
    pixel: &mut [T; N],
    targets: [f64; N],
    a: f64,
    round: impl Fn(f64) -> T,
    widen: impl Fn(T) -> f64,
) {
    if a == 0.0 {
        return;
    }
    for (out, target) in pixel.iter_mut().zip(targets) {
        *out = round((1.0 - a) * widen(*out) + a * target);
    }
}

/// Dynamic N16 stores of a constant colour's `targets` at `coverage`, at
/// the opacity `c / 255 · alpha` a span's coverage and constant alpha give
/// it. Returns the pixels from the first to past the last one with
/// coverage, if any. `pixels` and `coverage` have equal lengths.
pub(crate) fn dynamic_n16_runs(
    pixels: &mut [[u16; 4]],
    (targets, alpha): ([f64; 4], f64),
    coverage: &[u8],
) -> Option<(usize, usize)> {
    let first = coverage.iter().position(|&c| c != 0)?;
    let last = coverage.iter().rposition(|&c| c != 0).unwrap_or(first);
    let (pixels, coverage) = (pixels.get_mut(first..=last)?, coverage.get(first..=last)?);
    for (pixel, &c) in pixels.iter_mut().zip(coverage) {
        mix_dynamic(
            pixel,
            targets,
            UNIT[usize::from(c)] * alpha,
            round_u16,
            f64::from,
        );
    }
    Some((first, last + 1))
}

/// [`dynamic_n16_runs`] with one coverage byte `c` for every pixel.
pub(crate) fn dynamic_n16_uniform(
    pixels: &mut [[u16; 4]],
    (targets, alpha): ([f64; 4], f64),
    c: u8,
) {
    let a = UNIT[usize::from(c)] * alpha;
    for pixel in pixels {
        mix_dynamic(pixel, targets, a, round_u16, f64::from);
    }
}

/// `c / 255` for every coverage byte, as the division gives it.
pub(crate) const UNIT: [f64; 256] = {
    let mut unit = [0.0; 256];
    let mut c = 0;
    while c < 256 {
        unit[c] = c as f64 / 255.0;
        c += 1;
    }
    unit
};

/// [`store_f32`] of a constant colour's `target` levels at one coverage
/// byte `c` for every pixel: the opacity `c / 255 · alpha` a span's
/// coverage and constant alpha give it.
pub(crate) fn store_f32_uniform<const N: usize>(
    pixels: &mut [[f32; N]],
    (target, alpha): ([f32; N], f64),
    c: u8,
) {
    let alpha = single(UNIT[usize::from(c)] * alpha);
    for pixel in pixels {
        lerp(pixel, target, alpha);
    }
}

/// [`store_f32_uniform`] at `coverage`, which leaves pixels at zero as they
/// are. Returns the pixels from the first to past the last one with
/// coverage, if any. `pixels` and `coverage` have equal lengths.
pub(crate) fn store_f32_runs<const N: usize>(
    pixels: &mut [[f32; N]],
    (target, alpha): ([f32; N], f64),
    coverage: &[u8],
) -> Option<(usize, usize)> {
    let first = coverage.iter().position(|&c| c != 0)?;
    let last = coverage.iter().rposition(|&c| c != 0).unwrap_or(first);
    let (pixels, coverage) = (pixels.get_mut(first..=last)?, coverage.get(first..=last)?);
    for (pixel, &c) in pixels.iter_mut().zip(coverage) {
        lerp(pixel, target, single(UNIT[usize::from(c)] * alpha));
    }
    Some((first, last + 1))
}

/// Stores `colors` at `alpha` into F32 levels on 0..255: full opacity
/// overwrites, other stores evaluate `x += a·(t − x)` in f32.
pub(crate) fn store_f32<const N: usize>(
    pixels: &mut [[f32; N]],
    colors: &Colors<'_>,
    alpha: &[f64],
) {
    if let Colors::Strided { values, stride } = *colors
        && stride == N - 1
    {
        // At full opacity the store is the target's levels.
        let one = |pixel: &mut [f32; N], color: &[f64], a: f64| {
            lerp(pixel, levels(color), single(a));
        };
        by_runs((pixels, values, alpha), overwrite_f32, one);
        return;
    }
    match *colors {
        Colors::Constant(color) => {
            let target = levels(color);
            for (pixel, &a) in pixels.iter_mut().zip(alpha) {
                lerp(pixel, target, single(a));
            }
        }
        Colors::Strided { values, stride } => {
            for ((pixel, color), &a) in pixels
                .iter_mut()
                .zip(values.chunks(stride.max(1)))
                .zip(alpha)
            {
                lerp(pixel, levels(color), single(a));
            }
        }
    }
}

/// The levels of opaque colours, a value per colour lane, over `pixels`: a
/// store at full opacity, [`levels`] of each colour. Eight pixels at a time,
/// their values convert as one run, which vectorizes.
fn overwrite_f32<const N: usize>(pixels: &mut [[f32; N]], values: &[f64]) {
    let c = N - 1;
    let (blocks, last) = pixels.as_chunks_mut::<8>();
    let colors = values.chunks_exact(8 * c);
    let rest = colors.remainder();
    for (block, color) in blocks.iter_mut().zip(colors) {
        let mut levels = [0.0; 8 * (MAX_LANES - 1)];
        for (out, &value) in levels.iter_mut().zip(color) {
            *out = level(value);
        }
        for (pixel, levels) in block.iter_mut().zip(levels.chunks_exact(c)) {
            *pixel = [255.0; N];
            pixel[..c].copy_from_slice(levels);
        }
    }
    for (pixel, color) in last.iter_mut().zip(rest.chunks_exact(c)) {
        *pixel = levels(color);
    }
}

/// The most lanes of an F32 pixel: CMYK and alpha.
const MAX_LANES: usize = 5;

/// The F32 levels of an opaque colour: a level per colour lane, then full
/// alpha.
pub(crate) fn levels<const N: usize>(color: &[f64]) -> [f32; N] {
    let mut levels = [255.0; N];
    for (i, out) in levels.iter_mut().take(N - 1).enumerate() {
        *out = level(color.get(i).copied().unwrap_or(0.0));
    }
    levels
}

fn lerp<const N: usize>(pixel: &mut [f32; N], target: [f32; N], alpha: f32) {
    if alpha == 1.0 {
        *pixel = target;
        return;
    }
    for (out, target) in pixel.iter_mut().zip(target) {
        *out += alpha * (target - *out);
    }
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "an opacity or level narrowed to the admitted f32 storage"
)]
fn single(value: f64) -> f32 {
    value as f32
}

/// The F32 level of a unit value, rounded to odd when inexact so that a
/// following nearest-byte rounding keeps its side of a half level.
pub(crate) fn level(value: f64) -> f32 {
    let value = value.clamp(0.0, 1.0) * 255.0;
    let nearest = single(value);
    let truncated = nearest.to_bits() - u32::from(f64::from(nearest) > value);
    f32::from_bits(truncated | u32::from(f64::from(nearest) != value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_by_stretches_match_stores_by_pixel() {
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let words = [3, 30_000, 50_000, 65_535];
        for case in 0..2_000 {
            // Rows of zero, full and partial stretches of any length, so
            // that blocks of eight and the tail both see every kind.
            let len = usize::try_from(next() % 40).unwrap();
            let mut coverage = Vec::with_capacity(len);
            while coverage.len() < len {
                let run = 1 + usize::try_from(next() % 12).unwrap();
                let kind = next() % 3;
                for _ in 0..run.min(len - coverage.len()) {
                    coverage.push(match kind {
                        0 => 0,
                        1 => u8::MAX,
                        _ => u8::try_from(next() % 256).unwrap(),
                    });
                }
            }
            let pixels: Vec<[u16; 4]> = (0..len)
                .map(|_| [0; 4].map(|_: u16| u16::try_from(next() % 65_536).unwrap()))
                .collect();
            let (mut runs, mut each) = (pixels.clone(), pixels);
            let covered = store_n16_runs(&mut runs, words, &coverage);
            store_n16(&mut each, words, Some(&coverage));
            assert_eq!(runs, each, "{case}");
            let first = coverage.iter().position(|&c| c != 0);
            let last = coverage.iter().rposition(|&c| c != 0);
            assert_eq!(covered, first.zip(last).map(|(a, b)| (a, b + 1)), "{case}");
        }
    }

    #[test]
    fn uniform_coverage_stores_as_per_pixel_coverage() {
        let pixels: Vec<[u16; 4]> = (0..64_u16)
            .map(|i| [i.wrapping_mul(1021), i.wrapping_mul(4093), 65_535 - i, i])
            .collect();
        let words = [0, 1_234, 40_000, 65_535];
        for c in 0..=u8::MAX {
            let (mut uniform, mut each) = (pixels.clone(), pixels.clone());
            store_n16_uniform(&mut uniform, words, c);
            store_n16(&mut each, words, Some(&[c; 64]));
            assert_eq!(uniform, each, "coverage {c}");
        }
    }

    #[test]
    fn words_round_to_their_nearest_byte_in_u16_lanes() {
        for word in 0..=u16::MAX {
            let exact = (f64::from(word) / 257.0).round();
            assert_eq!(f64::from(word_byte(word)), exact, "word {word}");
        }
    }

    #[test]
    fn words_round_to_the_nearest_byte_at_the_output() {
        for step in 0..=10_000_u32 {
            let value = f64::from(step) / 10_000.0;
            let word = word(value);
            let byte = (u32::from(word) + 128) / 257;
            let exact = value * 255.0;
            assert_eq!(f64::from(byte), exact.round(), "{value}");
            assert!((f64::from(word) - exact * 257.0).abs() <= 1.0 + 1e-9);
        }
    }

    #[test]
    fn full_opacity_spans_store_as_any_span_does() {
        let values = [
            0.0,
            1.0,
            0.5,
            -0.25,
            1.5,
            f64::NAN,
            127.5 / 255.0,
            0.2,
            0.999_999,
            1.0 / 131_070.0,
            3.0 / 131_070.0,
            1e-300,
        ];
        // Four values a pixel take the general loop.
        let padded: Vec<f64> = values
            .chunks(3)
            .flat_map(|c| [c[0], c[1], c[2], 0.0])
            .collect();
        let alpha = [1.0; 4];
        let (fast, general) = (
            Colors::Strided {
                values: &values,
                stride: 3,
            },
            Colors::Strided {
                values: &padded,
                stride: 4,
            },
        );
        let (mut a, mut b) = ([[7_u16; 4]; 4], [[7_u16; 4]; 4]);
        dynamic_n16(&mut a, &fast, &alpha);
        dynamic_n16(&mut b, &general, &alpha);
        assert_eq!(a, b);
    }

    #[test]
    fn stores_by_runs_are_stores_by_pixel() {
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let unit = |random: u64| f64::from(u32::try_from(random % 1_000_001).unwrap_or(0)) / 1e6;
        for case in 0..400 {
            let len = case % 37;
            // Runs of full opacity between other opacities, zero included.
            let alpha: Vec<f64> = (0..len)
                .map(|k| match (k / 5 + case) % 3 {
                    0 => 1.0,
                    1 => unit(next()),
                    _ => [0.0, 1.0, 0.5][k % 3],
                })
                .collect();
            let colors: Vec<f64> = (0..len * 3).map(|_| unit(next()) * 1.2 - 0.1).collect();
            // Four values a pixel take the general loops.
            let padded: Vec<f64> = colors
                .chunks(3)
                .flat_map(|c| [c[0], c[1], c[2], 0.0])
                .collect();
            let (runs, general) = (
                Colors::Strided {
                    values: &colors,
                    stride: 3,
                },
                Colors::Strided {
                    values: &padded,
                    stride: 4,
                },
            );
            let words: Vec<[u16; 4]> = (0..len)
                .map(|_| [0; 4].map(|_: u16| u16::try_from(next() % 65_536).unwrap_or(0)))
                .collect();
            let (mut a, mut b) = (words.clone(), words);
            dynamic_n16(&mut a, &runs, &alpha);
            dynamic_n16(&mut b, &general, &alpha);
            assert_eq!(a, b, "n16 {case}");
            let levels: Vec<[f32; 4]> = (0..len)
                .map(|_| [0; 4].map(|_: u8| f32::from(u8::try_from(next() % 256).unwrap_or(0))))
                .collect();
            let bits = |pixels: &[[f32; 4]]| -> Vec<[u32; 4]> {
                pixels.iter().map(|p| p.map(f32::to_bits)).collect()
            };
            let (mut a, mut b) = (levels.clone(), levels.clone());
            dynamic_f32(&mut a, &runs, &alpha);
            dynamic_f32(&mut b, &general, &alpha);
            assert_eq!(bits(&a), bits(&b), "dynamic f32 {case}");
            let (mut a, mut b) = (levels.clone(), levels);
            store_f32(&mut a, &runs, &alpha);
            store_f32(&mut b, &general, &alpha);
            assert_eq!(bits(&a), bits(&b), "f32 {case}");
        }
    }

    #[test]
    fn cmyk_stores_by_runs_are_stores_by_pixel() {
        // Five lanes: four colour values a pixel take the runs, five the
        // general loops, and both give the same levels.
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let unit = |random: u64| f64::from(u32::try_from(random % 1_000_001).unwrap_or(0)) / 1e6;
        for case in 0..400 {
            let len = case % 37;
            let alpha: Vec<f64> = (0..len)
                .map(|k| match (k / 5 + case) % 3 {
                    0 => 1.0,
                    1 => unit(next()),
                    _ => [0.0, 1.0, 0.5][k % 3],
                })
                .collect();
            let colors: Vec<f64> = (0..len * 4).map(|_| unit(next()) * 1.2 - 0.1).collect();
            let padded: Vec<f64> = colors
                .chunks(4)
                .flat_map(|c| [c[0], c[1], c[2], c[3], 0.0])
                .collect();
            let (runs, general) = (
                Colors::Strided {
                    values: &colors,
                    stride: 4,
                },
                Colors::Strided {
                    values: &padded,
                    stride: 5,
                },
            );
            let levels: Vec<[f32; 5]> = (0..len)
                .map(|_| [0; 5].map(|_: u8| f32::from(u8::try_from(next() % 256).unwrap_or(0))))
                .collect();
            let bits = |pixels: &[[f32; 5]]| -> Vec<[u32; 5]> {
                pixels.iter().map(|p| p.map(f32::to_bits)).collect()
            };
            let (mut a, mut b) = (levels.clone(), levels.clone());
            dynamic_f32(&mut a, &runs, &alpha);
            dynamic_f32(&mut b, &general, &alpha);
            assert_eq!(bits(&a), bits(&b), "dynamic f32 {case}");
            let (mut a, mut b) = (levels.clone(), levels);
            store_f32(&mut a, &runs, &alpha);
            store_f32(&mut b, &general, &alpha);
            assert_eq!(bits(&a), bits(&b), "f32 {case}");
        }
    }

    #[test]
    fn full_opacity_f32_spans_store_as_any_span_does() {
        let values = [
            0.0,
            1.0,
            0.5,
            -0.25,
            1.5,
            f64::NAN,
            127.5 / 255.0,
            0.2,
            0.999_999,
            0.123,
            1e-300,
        ];
        // Eleven pixels: a block of eight and a tail of three.
        let colors: Vec<f64> = values.iter().cycle().take(33).copied().collect();
        let padded: Vec<f64> = colors
            .chunks(3)
            .flat_map(|c| [c[0], c[1], c[2], 0.0])
            .collect();
        let alpha = [1.0; 11];
        let (mut a, mut b) = ([[7.0_f32; 4]; 11], [[7.0_f32; 4]; 11]);
        store_f32(
            &mut a,
            &Colors::Strided {
                values: &colors,
                stride: 3,
            },
            &alpha,
        );
        store_f32(
            &mut b,
            &Colors::Strided {
                values: &padded,
                stride: 4,
            },
            &alpha,
        );
        let bits = |pixels: [[f32; 4]; 11]| pixels.map(|p| p.map(f32::to_bits));
        assert_eq!(bits(a), bits(b));
    }

    #[test]
    fn integer_identities_hold_on_their_whole_domains() {
        for x in 0..=255 * 65_535 + 127_u32 {
            assert_eq!(div255(x), x / 255, "{x}");
        }
        for word in 0..=u16::MAX {
            assert_eq!(
                u32::from(word_byte(word)),
                (u32::from(word) + 128) / 257,
                "{word}"
            );
        }
    }

    #[test]
    fn n16_mixes_once_per_store() {
        let mut pixels = [[65535_u16; 4]; 3];
        store_n16(&mut pixels, [0, 0, 0, 65535], Some(&[0, 128, 255]));
        assert_eq!(pixels[0], [65535; 4]);
        let mixed = u16::try_from((65535 * 127 + 127) / 255).unwrap();
        assert_eq!(pixels[1], [mixed, mixed, mixed, 65535]);
        assert_eq!(pixels[2], [0, 0, 0, 65535]);
    }

    #[test]
    fn levels_round_to_odd_when_inexact() {
        assert_eq!(level(0.5), 127.5);
        assert_eq!(level(1.0), 255.0);
        let inexact = level(0.123);
        assert_eq!(inexact.to_bits() & 1, 1);
    }

    #[test]
    fn f32_stores_source_over() {
        let mut pixels = [[255.0_f32; 4]];
        store_f32(&mut pixels, &Colors::Constant(&[0.0, 0.0, 0.0]), &[0.5]);
        assert_eq!(pixels[0], [127.5, 127.5, 127.5, 255.0]);
    }
}
