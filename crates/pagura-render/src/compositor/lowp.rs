//! Portable one-round byte source-over for [`crate::Accuracy::Competitive`].
//! Weighted byte sums are at most 65025; adding the half-rounding bias and
//! its high byte remains below 65536. Wrapping lane arithmetic cannot wrap,
//! neither in 16-bit lanes nor in the 16-bit halves of a 32-bit word.

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
                .wrapping_add(u16::from(*out).wrapping_mul(255u16.wrapping_sub(a))),
        ) as u8;
    }
}

/// A source colour and alpha prepared for [`row`] and [`run`].
#[derive(Debug, Clone, Copy)]
pub(super) struct Source {
    /// Four pixels' lanes.
    color: [u16; 16],
    /// Channels 0 and 2, and 1 and 3, in the halves of a word.
    even: u32,
    odd: u32,
    pixel: [u8; 4],
    alpha: u8,
}

impl Source {
    pub(super) fn new(pixel: [u8; 4], alpha: u8) -> Self {
        let mut color = [0; 16];
        for (lanes, &value) in color.iter_mut().zip(pixel.iter().cycle()) {
            *lanes = u16::from(value);
        }
        let word = u32::from_le_bytes(pixel);
        Self {
            color,
            even: word & HALVES,
            odd: (word >> 8) & HALVES,
            pixel,
            alpha,
        }
    }
}

/// The low byte of each 16-bit half of a word.
const HALVES: u32 = 0x00ff_00ff;

/// Source-over of `source` at its alpha times each coverage, rounding each
/// store once. Four pixels at a time, each a word whose channel pairs are
/// mixed in its halves, so that every target's vectorizer sees eight
/// independent products of one form; at full alpha the coverage is the
/// weight.
pub(super) fn row(target: &mut [[u8; 4]], coverage: &[u8], source: &Source) {
    rows(std::iter::once((target, coverage)), source);
}

/// [`row`] for each target row and its coverage, as one mask's rows.
#[inline(never)]
pub(super) fn rows<'a>(rows: impl Iterator<Item = (&'a mut [[u8; 4]], &'a [u8])>, source: &Source) {
    if source.alpha == 255 {
        for (target, coverage) in rows {
            blocks(target, coverage, source, |block, mask| {
                blend(block, source, words(mask));
            });
        }
    } else {
        let alpha = u32::from(source.alpha);
        for (target, coverage) in rows {
            blocks(target, coverage, source, |block, mask| {
                // The product lies in the low half; the high half stays zero.
                let opacity = words(mask).map(|c| halves(c.wrapping_mul(alpha)));
                blend(block, source, opacity);
            });
        }
    }
}

/// Applies `paint` to each block of four pixels with some coverage, and
/// source-over to the pixels past the last whole block.
#[inline(always)]
#[expect(
    clippy::cast_possible_truncation,
    clippy::inline_always,
    reason = "rounded byte coverage fits in a byte; one call per row of a mask"
)]
fn blocks(
    target: &mut [[u8; 4]],
    coverage: &[u8],
    source: &Source,
    mut paint: impl FnMut(&mut [[u8; 4]; 4], [u8; 4]),
) {
    let (blocks, tail) = target.as_chunks_mut::<4>();
    let (masks, mask_tail) = coverage.as_chunks::<4>();
    // Transparent coverage leaves a pixel unchanged.
    for (block, &mask) in blocks.iter_mut().zip(masks) {
        if mask != [0; 4] {
            paint(block, mask);
        }
    }
    for (out, &coverage) in tail.iter_mut().zip(mask_tail) {
        pixel(
            out,
            source.pixel,
            divide(u16::from(coverage).wrapping_mul(u16::from(source.alpha))) as u8,
        );
    }
}

/// Stores `source` over a run of one `coverage`: the source itself at full
/// coverage when opaque. The source products are formed once for the run,
/// and four pixels are blended at a time.
#[expect(
    clippy::cast_possible_truncation,
    reason = "rounded byte coverage and convex byte mixtures fit in a byte"
)]
pub(super) fn run(target: &mut [[u8; 4]], coverage: u8, source: &Source) {
    let opacity = divide(u16::from(coverage).wrapping_mul(u16::from(source.alpha)));
    if opacity == 255 {
        target.fill(source.pixel);
        return;
    }
    let mut products = [0u16; 16];
    for (product, &color) in products.iter_mut().zip(&source.color) {
        *product = color.wrapping_mul(opacity);
    }
    let rest = 255u16.wrapping_sub(opacity);
    let (blocks, tail) = target.as_chunks_mut::<4>();
    for block in blocks {
        for (out, &product) in block.as_flattened_mut().iter_mut().zip(&products) {
            *out = divide(product.wrapping_add(u16::from(*out).wrapping_mul(rest))) as u8;
        }
    }
    for out in tail {
        pixel(out, source.pixel, opacity as u8);
    }
}

/// Each of four pixels' levels in a word.
#[inline(always)]
#[expect(clippy::inline_always, reason = "one block per four pixels")]
fn words(mask: [u8; 4]) -> [u32; 4] {
    let word = u32::from_le_bytes(mask);
    [0, 8, 16, 24].map(|shift| (word >> shift) & 0xff)
}

/// [`divide`] in both halves of a word.
#[inline(always)]
#[expect(clippy::inline_always, reason = "two per pixel")]
fn halves(value: u32) -> u32 {
    let value = value.wrapping_add(0x0080_0080);
    (value.wrapping_add((value >> 8) & HALVES) >> 8) & HALVES
}

/// Four pixels of source-over at per-pixel opacities of at most 255. All
/// lane arithmetic wraps, so that it vectorizes with overflow checks on.
#[inline(always)]
#[expect(clippy::inline_always, reason = "one block per four pixels")]
fn blend(block: &mut [[u8; 4]; 4], source: &Source, opacity: [u32; 4]) {
    let mut pixels = block.map(u32::from_le_bytes);
    for (pixel, opacity) in pixels.iter_mut().zip(opacity) {
        let rest = 255u32.wrapping_sub(opacity);
        let even = (*pixel & HALVES)
            .wrapping_mul(rest)
            .wrapping_add(source.even.wrapping_mul(opacity));
        let odd = ((*pixel >> 8) & HALVES)
            .wrapping_mul(rest)
            .wrapping_add(source.odd.wrapping_mul(opacity));
        *pixel = halves(even) | (halves(odd) << 8);
    }
    *block = pixels.map(u32::to_le_bytes);
}

/// Multiplies byte coverages, rounding each product once. Full and empty
/// 16-byte blocks of `factors` take no multiplication.
#[expect(
    clippy::cast_possible_truncation,
    reason = "a rounded byte product is at most 255"
)]
pub(super) fn multiply(target: &mut [u8], factors: &[u8]) {
    let (blocks, tail) = target.as_chunks_mut::<16>();
    let (masks, mask_tail) = factors.as_chunks::<16>();
    for (block, mask) in blocks.iter_mut().zip(masks) {
        if *mask == [255; 16] {
            continue;
        }
        if *mask == [0; 16] {
            block.fill(0);
            continue;
        }
        for (out, &mask) in block.iter_mut().zip(mask) {
            *out = divide(u16::from(*out).wrapping_mul(u16::from(mask))) as u8;
        }
    }
    for (out, &mask) in tail.iter_mut().zip(mask_tail) {
        *out = divide(u16::from(*out) * u16::from(mask)) as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// A different byte in each channel, each channel taking every value
    /// as `value` does, so that a channel mixed in another's place shows.
    fn channels(value: u8) -> [u8; 4] {
        [0x00, 0x55, 0xaa, 0xff].map(|key| value ^ key)
    }

    #[test]
    fn portable_lanes_round_every_byte_mixture_once() {
        for alpha in 0..=255u8 {
            for source in 0..=255u8 {
                let mut pixels: Vec<_> = (0..=255u8).map(channels).collect();
                row(
                    &mut pixels,
                    &[alpha; 256],
                    &Source::new(channels(source), 255),
                );
                for (background, result) in (0..=255u8).zip(&pixels) {
                    let (source, background) = (channels(source), channels(background));
                    let expected = [0, 1, 2, 3].map(|k| {
                        let mixed = u32::from(source[k]) * u32::from(alpha)
                            + u32::from(background[k]) * (255 - u32::from(alpha))
                            + 127;
                        u8::try_from(mixed / 255).unwrap()
                    });
                    assert_eq!(result, &expected);
                }
            }
        }
    }

    #[test]
    fn coverage_and_alpha_round_their_product_first() {
        for alpha in [0, 1, 128, 254, 255] {
            for length in 0..11 {
                let byte = |i: usize, step: usize| u8::try_from(i * step % 256).unwrap();
                let coverage: Vec<u8> = (0..length).map(|i| byte(i, 37)).collect();
                let lanes: Vec<_> = (0..length).map(|i| channels(byte(i, 23))).collect();
                for color in [[200, 10, 90, 255], [0, 0, 0, 255]] {
                    let mut lanes = lanes.clone();
                    let mut single = lanes.clone();
                    row(&mut lanes, &coverage, &Source::new(color, alpha));
                    for (target, &c) in single.iter_mut().zip(&coverage) {
                        let opacity = (u16::from(c) * u16::from(alpha) + 127) / 255;
                        pixel(target, color, u8::try_from(opacity).unwrap());
                    }
                    assert_eq!(lanes, single, "{alpha} {length} {color:?}");
                }
            }
        }
    }

    #[test]
    fn uniform_runs_blend_as_single_pixels() {
        for alpha in [0, 1, 128, 255] {
            for coverage in [0, 1, 77, 254, 255] {
                for length in 0..11 {
                    let mut runs: Vec<_> = (0..length)
                        .map(|i| [u8::try_from(i * 41 % 256).unwrap(), 3, 250, 255])
                        .collect();
                    let mut single = runs.clone();
                    let source = Source::new([12, 200, 99, 255], alpha);
                    run(&mut runs, coverage, &source);
                    row(&mut single, &vec![coverage; length], &source);
                    assert_eq!(runs, single, "{alpha} {coverage} {length}");
                }
            }
        }
    }

    #[test]
    fn products_keep_every_tail_and_uniform_block() {
        for size in 0..=97 {
            for mask in [0, 1, 127, 128, 254, 255] {
                let mut pixels: Vec<_> = (0..size)
                    .map(|i| u8::try_from((i * 73 + 29) % 256).unwrap())
                    .collect();
                let factors: Vec<_> = (0..size)
                    .map(|i| match (i / 16) % 3 {
                        0 => 255,
                        1 => 0,
                        _ => mask,
                    })
                    .collect();
                let expected: Vec<_> = pixels
                    .iter()
                    .zip(&factors)
                    .map(|(&a, &b)| {
                        u8::try_from((u32::from(a) * u32::from(b) + 127) / 255).unwrap()
                    })
                    .collect();
                multiply(&mut pixels, &factors);
                assert_eq!(pixels, expected);
            }
        }
    }
}
