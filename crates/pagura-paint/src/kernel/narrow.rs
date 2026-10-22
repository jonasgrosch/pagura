//! Normal source-over into narrow RGBA storage (ADR 0004 §5–§6): N16 rounds
//! once per store of an opaque colour at a coverage byte; a dynamic store
//! (any other opacity, or a child's result) is evaluated in f64 and rounded
//! once to the buffer's format; F32 mark stores evaluate `x += a·(t − x)`;
//! U8 rounds every store to bytes and is used only under the `byte_storage`
//! fast parameter. Colours are premultiplied with the background embedded.

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

/// Stores an opaque colour, already encoded as bytes, at `coverage` (`None`
/// covers every pixel): `round((c·s + (255 − c)·u) / 255)`, the store the
/// byte path makes at alpha `c / 255`, with no branch.
pub(crate) fn store_u8_opaque(pixels: &mut [[u8; 4]], source: [u8; 4], coverage: Option<&[u8]>) {
    let Some(coverage) = coverage else {
        pixels.fill(source);
        return;
    };
    let source = Lanes::new(source);
    for (pixel, &c) in pixels.iter_mut().zip(coverage) {
        *pixel = source.over(*pixel, u32::from(c));
    }
}

/// Stores a constant colour, encoded as bytes, at the byte alpha `alpha[c]`
/// of each pixel's coverage byte `c` (`None` covers every pixel), the store
/// the byte path makes once the opacity is a byte.
pub(crate) fn store_u8_alpha(
    pixels: &mut [[u8; 4]],
    source: [u8; 4],
    coverage: Option<&[u8]>,
    alpha: &[u8; 256],
) {
    let source = Lanes::new(source);
    let Some(coverage) = coverage else {
        let a = u32::from(alpha[255]);
        for pixel in pixels.iter_mut() {
            *pixel = source.over(*pixel, a);
        }
        return;
    };
    for (pixel, &c) in pixels.iter_mut().zip(coverage) {
        *pixel = source.over(*pixel, u32::from(alpha[usize::from(c)]));
    }
}

/// Stores pixels of an image given as bytes (`[0, 0, 0, 0]` where it paints
/// nothing) at the byte alpha `alpha[c]` of each pixel's coverage byte `c`
/// (`None` covers every pixel), the store the byte path makes.
pub(crate) fn store_u8_bytes(
    pixels: &mut [[u8; 4]],
    sources: &[[u8; 4]],
    coverage: Option<&[u8]>,
    alpha: &[u8; 256],
) {
    let one = |pixel: &mut [u8; 4], source: [u8; 4], a: u8| {
        if source[3] == 0 {
            return;
        }
        match a {
            255 => *pixel = source,
            0 => {}
            a => *pixel = Lanes::new(source).over(*pixel, u32::from(a)),
        }
    };
    match coverage {
        None => {
            let a = alpha[255];
            for (pixel, &source) in pixels.iter_mut().zip(sources) {
                one(pixel, source, a);
            }
        }
        Some(coverage) => {
            for ((pixel, &source), &c) in pixels.iter_mut().zip(sources).zip(coverage) {
                one(pixel, source, alpha[usize::from(c)]);
            }
        }
    }
}

/// A byte colour as two pairs of channels in the 16-bit halves of a word
/// each, so that one multiply weights two channels: a weighted channel is
/// at most `255² + 128 + 254 < 2^16`, so no half carries into the other.
#[derive(Clone, Copy)]
struct Lanes {
    /// Channels 0 and 2.
    even: u32,
    /// Channels 1 and 3.
    odd: u32,
}

/// The low byte of each 16-bit half of a word.
const HALVES: u32 = 0x00ff_00ff;

impl Lanes {
    fn new(color: [u8; 4]) -> Self {
        let word = u32::from_le_bytes(color);
        Self {
            even: word & HALVES,
            odd: (word >> 8) & HALVES,
        }
    }

    /// `round255(s·a + u·(255 − a))` per channel of `pixel`.
    #[inline]
    fn over(self, pixel: [u8; 4], a: u32) -> [u8; 4] {
        let word = u32::from_le_bytes(pixel);
        let rest = 255_u32.wrapping_sub(a);
        let even = round255_halves(
            self.even
                .wrapping_mul(a)
                .wrapping_add((word & HALVES).wrapping_mul(rest)),
        );
        let odd = round255_halves(
            self.odd
                .wrapping_mul(a)
                .wrapping_add(((word >> 8) & HALVES).wrapping_mul(rest)),
        );
        (even | (odd << 8)).to_le_bytes()
    }
}

/// `round255` in both 16-bit halves of a word.
#[inline]
fn round255_halves(p: u32) -> u32 {
    let t = p.wrapping_add(0x0080_0080);
    (t.wrapping_add((t >> 8) & HALVES) >> 8) & HALVES
}

/// The byte opacity of every coverage byte under constant alpha `k`: the
/// byte path's `quantize(c / 255 · k)`.
pub(crate) fn alpha_bytes(k: f64) -> [u8; 256] {
    let mut table = [0; 256];
    for (c, out) in table.iter_mut().enumerate() {
        let unit = crate::execute::unit(c);
        *out = pagura_raster::quantize_unit(unit * k);
    }
    table
}

/// The bytes the byte path stores for an opaque constant colour.
pub(crate) fn color_bytes(color: &[f64]) -> [u8; 4] {
    bytes(color)
}

/// `round(p / 255)` for `p ≤ 255²` (never a tie): `t = p + 128`, then
/// `(t + (t >> 8)) >> 8`.
#[cfg(test)]
pub(crate) fn round255(p: u32) -> u32 {
    let t = p.wrapping_add(128);
    t.wrapping_add(t >> 8) >> 8
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
    dynamic(
        pixels,
        colors,
        alpha,
        65535.0,
        |value| {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a value clamped to the word range"
            )]
            let word = value.clamp(0.0, 65535.0).round() as u16;
            word
        },
        f64::from,
    );
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

/// Stores `colors` at `alpha` into bytes, rounding each store once.
pub(crate) fn store_u8(pixels: &mut [[u8; 4]], colors: &Colors<'_>, alpha: &[f64]) {
    match *colors {
        Colors::Constant(color) => {
            let source = bytes(color);
            for (pixel, &a) in pixels.iter_mut().zip(alpha) {
                over(pixel, source, a);
            }
        }
        Colors::Strided { values, stride } => {
            for ((pixel, color), &a) in pixels
                .iter_mut()
                .zip(values.chunks(stride.max(1)))
                .zip(alpha)
            {
                over(pixel, bytes(color), a);
            }
        }
    }
}

fn bytes(color: &[f64]) -> [u8; 4] {
    let at = |i: usize| pagura_raster::quantize_unit(color.get(i).copied().unwrap_or(0.0));
    [at(0), at(1), at(2), 255]
}

/// One byte store at alpha `quantize(alpha)`, rounded once: at 255 the
/// source, at 0 the pixel as it is, which is what the rounding gives too.
fn over(pixel: &mut [u8; 4], source: [u8; 4], alpha: f64) {
    match pagura_raster::quantize_unit(alpha) {
        255 => *pixel = source,
        0 => {}
        a => *pixel = Lanes::new(source).over(*pixel, u32::from(a)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One byte store channel by channel, as the byte path defines it.
    fn reference_over(pixel: &mut [u8; 4], source: [u8; 4], alpha: f64) {
        let a = u32::from(pagura_raster::quantize_unit(alpha));
        let rest = 255 - a;
        for (out, source) in pixel.iter_mut().zip(source) {
            let sum = u32::from(source) * a + u32::from(*out) * rest;
            *out = u8::try_from(round255(sum)).unwrap();
        }
    }

    #[test]
    fn byte_stores_round_once_per_channel() {
        for a in 0..=255_u8 {
            let alpha = f64::from(a) / 255.0;
            for s in (0..=255_u8).step_by(3) {
                for b in (0..=255_u8).step_by(7) {
                    let mut fast = [b, 255 - b, b / 2, 255];
                    let mut slow = fast;
                    over(&mut fast, [s, 255 - s, s / 3, 255], alpha);
                    reference_over(&mut slow, [s, 255 - s, s / 3, 255], alpha);
                    assert_eq!(fast, slow, "a {a} s {s} b {b}");
                }
            }
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
    fn opaque_byte_stores_match_the_byte_path() {
        for c in 0..=255_u8 {
            let alpha = f64::from(c) / 255.0;
            assert_eq!(pagura_raster::quantize_unit(alpha), c);
            for s in [0_u8, 1, 77, 128, 254, 255] {
                for b in 0..=255_u8 {
                    let mut fast = [[b, b, b, b]];
                    store_u8_opaque(&mut fast, [s, s, s, 255], Some(&[c]));
                    let mut slow = [b, b, b, b];
                    reference_over(&mut slow, [s, s, s, 255], alpha);
                    assert_eq!(fast[0], slow, "s {s} b {b} c {c}");
                }
            }
        }
        let mut full = [[3_u8; 4]; 3];
        store_u8_opaque(&mut full, [9, 8, 7, 255], None);
        assert_eq!(full, [[9, 8, 7, 255]; 3]);
    }

    #[test]
    fn image_byte_stores_match_the_byte_path() {
        for k in [0.25, 1.0] {
            let table = alpha_bytes(k);
            for c in (0..=255_u8).step_by(3) {
                let alpha = crate::execute::unit(usize::from(c)) * k;
                let sources = [[200, 10, 90, 255], [0, 0, 0, 0], [7, 255, 0, 255]];
                let mut fast = [[30, 60, 90, 255], [1, 2, 3, 4], [255, 0, 255, 255]];
                let mut slow = fast;
                store_u8_bytes(&mut fast, &sources, Some(&[c; 3]), &table);
                for (pixel, source) in slow.iter_mut().zip(sources) {
                    if source[3] == 255 {
                        reference_over(pixel, source, alpha);
                    }
                }
                assert_eq!(fast, slow, "k {k} c {c}");
            }
            let sources = [[200, 10, 90, 255], [0, 0, 0, 0]];
            let mut fast = [[30, 60, 90, 255], [1, 2, 3, 4]];
            let mut slow = fast;
            store_u8_bytes(&mut fast, &sources, None, &table);
            reference_over(&mut slow[0], sources[0], k);
            assert_eq!(fast, slow, "k {k}");
        }
    }

    #[test]
    fn translucent_byte_stores_match_the_byte_path() {
        for k in [0.0, 0.001, 0.25, 0.4, 0.5, 0.75, 0.999, 1.0] {
            let table = alpha_bytes(k);
            for c in 0..=255_u8 {
                let alpha = crate::execute::unit(usize::from(c)) * k;
                for s in [0_u8, 90, 255] {
                    for b in (0..=255_u8).step_by(5) {
                        let mut fast = [[b; 4]];
                        store_u8_alpha(&mut fast, [s, s, s, 255], Some(&[c]), &table);
                        let mut slow = [b; 4];
                        reference_over(&mut slow, [s, s, s, 255], alpha);
                        assert_eq!(fast[0], slow, "k {k} s {s} b {b} c {c}");
                    }
                }
            }
            let mut full = [[40_u8; 4]; 2];
            store_u8_alpha(&mut full, [200, 100, 0, 255], None, &table);
            let mut slow = [40_u8; 4];
            reference_over(&mut slow, [200, 100, 0, 255], 1.0 * k);
            assert_eq!(full, [slow; 2], "k {k}");
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
        for p in 0..=255 * 255_u32 {
            assert_eq!(round255(p), (p + 127) / 255, "{p}");
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
    fn f32_and_u8_store_source_over() {
        let mut pixels = [[255.0_f32; 4]];
        store_f32(&mut pixels, &Colors::Constant(&[0.0, 0.0, 0.0]), &[0.5]);
        assert_eq!(pixels[0], [127.5, 127.5, 127.5, 255.0]);
        let mut bytes = [[255_u8; 4]];
        store_u8(&mut bytes, &Colors::Constant(&[0.0, 0.0, 0.0]), &[0.5]);
        assert_eq!(bytes[0], [127, 127, 127, 255]);
    }
}
