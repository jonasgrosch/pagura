//! Normal source-over into narrow RGBA storage (ADR 0004 §5–§6): N16 rounds
//! once per store of an opaque colour at a coverage byte; a dynamic store
//! (any other opacity, or a child's result) is evaluated in f64 and rounded
//! once to the buffer's format; F32 mark stores evaluate `x += a·(t − x)`;
//! U8 rounds every store to bytes and is used only under the `byte_storage`
//! fast parameter. Colours are premultiplied with the background embedded.

use super::Colors;

/// Stores an opaque colour, already encoded as N16 `words`, at `coverage`
/// (`None` covers every pixel): `floor((c·s + (255 − c)·u + 127) / 255)`
/// with one integer rounding per store.
pub(crate) fn store_n16(pixels: &mut [[u16; 4]], words: [u16; 4], coverage: Option<&[u8]>) {
    let Some(coverage) = coverage else {
        pixels.fill(words);
        return;
    };
    for (pixel, &c) in pixels.iter_mut().zip(coverage) {
        match c {
            0 => {}
            255 => *pixel = words,
            c => {
                for (out, &source) in pixel.iter_mut().zip(&words) {
                    *out = mix(source, *out, c);
                }
            }
        }
    }
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "a convex combination of two words stays within 0..65535"
)]
fn mix(source: u16, backdrop: u16, coverage: u8) -> u16 {
    let c = u32::from(coverage);
    ((u32::from(source) * c + u32::from(backdrop) * (255 - c) + 127) / 255) as u16
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
    dynamic(pixels, colors, alpha, 65535.0, |value| {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a value clamped to the word range"
        )]
        let word = value.clamp(0.0, 65535.0).round() as u16;
        word
    }, f64::from);
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

fn over(pixel: &mut [u8; 4], source: [u8; 4], alpha: f64) {
    let a = u32::from(pagura_raster::quantize_unit(alpha));
    if a == 0 {
        return;
    }
    if a == 255 {
        *pixel = source;
        return;
    }
    for (out, source) in pixel.iter_mut().zip(source) {
        let value = (u32::from(source) * a + u32::from(*out) * (255 - a) + 127) / 255;
        *out = u8::try_from(value).unwrap_or(u8::MAX);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
