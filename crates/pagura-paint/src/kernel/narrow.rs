//! Normal source-over into narrow RGBA storage (ADR 0004 §5–§6): N16 rounds
//! once per store of an opaque colour at a coverage byte; a dynamic store
//! (any other opacity, or a child's result) is evaluated in f64 and rounded
//! once to the buffer's format; F32 mark stores evaluate `x += a·(t − x)`.
//! Colours are premultiplied with the background embedded.

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
        *byte = level_byte(level);
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a level rounded and clamped to a byte"
)]
fn level_byte(level: f32) -> u8 {
    level.round().clamp(0.0, 255.0) as u8
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
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a clamped unit value scaled to 0..65535"
)]
pub(crate) fn word(value: f64) -> u16 {
    let level = value.clamp(0.0, 1.0) * 255.0;
    let word = (level * 257.0).round() as u16;
    let byte = (u32::from(word) + 128) / 257;
    let nearest = level.round() as u32;
    match byte.cmp(&nearest) {
        std::cmp::Ordering::Greater => word - 1,
        std::cmp::Ordering::Less => word + 1,
        std::cmp::Ordering::Equal => word,
    }
}

/// A dynamic store into N16 words: `(1 − a)·x + a·t` for the target
/// `t = 65535·c` evaluated in f64 and rounded once to the nearest word.
pub(crate) fn dynamic_n16(pixels: &mut [[u16; 4]], colors: &Colors<'_>, alpha: &[f64]) {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a value clamped to the word range"
    )]
    let round = |value: f64| value.clamp(0.0, 65535.0).round() as u16;
    if let Colors::Strided { values, stride: 3 } = *colors
        && full(alpha)
    {
        // At full opacity `(1 − a)·x + a·t` is `t`.
        overwrite_n16(pixels, values);
        return;
    }
    dynamic(pixels, colors, alpha, 65535.0, round, f64::from);
}

/// The word a store at full opacity writes for colour value `value`:
/// `65535·value` clamped and rounded. `max` and `min` clamp as `clamp`
/// does, with a NaN to zero, which is also its word.
#[inline]
#[expect(
    clippy::manual_clamp,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "unlike clamp, max and min vectorize, and NaN gives the same word"
)]
pub(crate) fn opaque_word(value: f64) -> u16 {
    (value.max(0.0).min(1.0) * 65535.0).round() as u16
}

/// The words of opaque colours, three values a pixel, over `pixels`: a
/// store at full opacity, the word of each target `65535·c`. `max` and
/// `min` clamp as `clamp` does, with a NaN to zero, which is also its word;
/// the target then needs no clamp to the word range. Eight pixels at a
/// time, their 24 values convert as one run, which vectorizes.
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
pub(crate) fn dynamic_f32(pixels: &mut [[f32; 4]], colors: &Colors<'_>, alpha: &[f64]) {
    dynamic(pixels, colors, alpha, 255.0, single, f64::from);
}

fn dynamic<T: Copy>(
    pixels: &mut [[T; 4]],
    colors: &Colors<'_>,
    alpha: &[f64],
    scale: f64,
    round: impl Fn(f64) -> T,
    widen: impl Fn(T) -> f64,
) {
    let store = |pixel: &mut [T; 4], color: &[f64], a: f64| {
        if a == 0.0 {
            return;
        }
        for (i, out) in pixel.iter_mut().enumerate() {
            let target = if i == 3 {
                scale
            } else {
                color.get(i).copied().unwrap_or(0.0).clamp(0.0, 1.0) * scale
            };
            *out = round((1.0 - a) * widen(*out) + a * target);
        }
    };
    match *colors {
        Colors::Constant(color) => {
            for (pixel, &a) in pixels.iter_mut().zip(alpha) {
                store(pixel, color, a);
            }
        }
        Colors::Strided { values, stride } => {
            for ((pixel, color), &a) in pixels
                .iter_mut()
                .zip(values.chunks(stride.max(1)))
                .zip(alpha)
            {
                store(pixel, color, a);
            }
        }
    }
}

/// Stores `colors` at `alpha` into F32 levels on 0..255: full opacity
/// overwrites, other stores evaluate `x += a·(t − x)` in f32.
pub(crate) fn store_f32(pixels: &mut [[f32; 4]], colors: &Colors<'_>, alpha: &[f64]) {
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

fn levels(color: &[f64]) -> [f32; 4] {
    let at = |i: usize| level(color.get(i).copied().unwrap_or(0.0));
    [at(0), at(1), at(2), 255.0]
}

fn lerp(pixel: &mut [f32; 4], target: [f32; 4], alpha: f32) {
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
