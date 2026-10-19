//! Byte source-over kernels, rounding once per store, on a tile of the page.

use super::{clip::ClipMask, coverage::Coverage};
use crate::Tile;
use pagura_core::Result;
use pagura_raster::{Pixmap, Rgba8};

/// A tile of the page; marks are positioned in page device pixels.
pub(super) struct Target {
    pixels: Pixmap,
    tile: Tile,
    background: Rgba8,
    /// Per tile row, the tile columns `[lo, hi)` that any mark has been
    /// given to paint; every other pixel still holds the background.
    painted: Vec<[u32; 2]>,
}

impl Target {
    pub(super) fn new(tile: Tile, background: Rgba8, limits: pagura_core::Limits) -> Result<Self> {
        let pixels = Pixmap::new_with_limits(tile.width, tile.height, background, limits)?;
        let mut painted = Vec::new();
        painted
            .try_reserve_exact(tile.height as usize)
            .map_err(|_| pagura_core::Error::limit("surface allocation failed"))?;
        painted.resize(tile.height as usize, [0; 2]);
        Ok(Self {
            pixels,
            tile,
            background,
            painted,
        })
    }

    pub(super) fn tile(&self) -> Tile {
        self.tile
    }

    pub(super) fn finish(self) -> Pixmap {
        self.pixels
    }

    /// The pixels of page row `y` from column `x0` (inclusive) to `x1`
    /// (exclusive) that lie in the tile, with the first column they start at.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "indices are clamped to the tile, whose extent fits usize"
    )]
    pub(super) fn span(&mut self, y: i64, x0: i64, x1: i64) -> Option<(i64, &mut [u8])> {
        let tile = self.tile;
        let row = y - i64::from(tile.y);
        if row < 0 || row >= i64::from(tile.height) {
            return None;
        }
        let lo = x0.max(i64::from(tile.x));
        let hi = x1.min(i64::from(tile.x) + i64::from(tile.width));
        if lo >= hi {
            return None;
        }
        let stride = tile.width as usize;
        let (left, right) = (
            (lo - i64::from(tile.x)) as u32,
            (hi - i64::from(tile.x)) as u32,
        );
        let painted = &mut self.painted[row as usize];
        *painted = if painted[0] == painted[1] {
            [left, right]
        } else {
            [painted[0].min(left), painted[1].max(right)]
        };
        let start = row as usize * stride + left as usize;
        let end = start + (hi - lo) as usize;
        Some((lo, &mut self.pixels.data_mut()[start * 4..end * 4]))
    }

    /// Whether every pixel of page row `y` from column `x0` (inclusive) to
    /// `x1` (exclusive) that lies in the tile still holds the background.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "indices are clamped to the tile, whose extent fits u32"
    )]
    fn unpainted(&self, y: i64, x0: i64, x1: i64) -> bool {
        let tile = self.tile;
        let row = y - i64::from(tile.y);
        if row < 0 || row >= i64::from(tile.height) {
            return true;
        }
        let lo = (x0 - i64::from(tile.x)).clamp(0, i64::from(tile.width)) as u32;
        let hi = (x1 - i64::from(tile.x)).clamp(0, i64::from(tile.width)) as u32;
        let [left, right] = self.painted[row as usize];
        lo >= hi || left == right || hi <= left || right <= lo
    }
}

impl Target {
    /// Calls `paint` with each pixel of page row `y` from column `x` for
    /// `width` columns that lies in the tile, and its column offset.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_possible_wrap,
        reason = "spans are clamped to the tile, whose extent fits usize"
    )]
    pub(super) fn paint_row(
        &mut self,
        y: i64,
        x: i64,
        width: usize,
        mut paint: impl FnMut(usize, &mut [u8; 4]),
    ) {
        if let Some((start, pixels)) = self.span(y, x, x + width as i64) {
            let skip = (start - x) as usize;
            for (column, pixel) in pixels.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                paint(skip + column, pixel);
            }
        }
    }
}

/// Paints `mask`, translated by `offset`, in `color` through `clip`.
#[expect(
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation,
    reason = "mask extents are bounded; spans are clamped to the tile before indexing"
)]
pub(super) fn blit(
    target: &mut Target,
    mask: &Coverage,
    offset: (i32, i32),
    color: Rgba8,
    clip: Option<&ClipMask>,
) {
    if mask.width == 0 {
        return;
    }
    let x = i64::from(mask.x) + i64::from(offset.0);
    let y = i64::from(mask.y) + i64::from(offset.1);
    let clip = clip.filter(|c| !c.covers(x, y, mask.width, mask.height()));
    for (row, coverage) in mask.data.chunks_exact(mask.width).enumerate() {
        let py = y + row as i64;
        let Some((start, pixels)) = target.span(py, x, x + mask.width as i64) else {
            continue;
        };
        let coverage = &coverage[(start - x) as usize..];
        if let Some(clip) = clip {
            for (column, (pixel, &alpha)) in pixels
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(coverage)
                .enumerate()
            {
                let c = clip.at(start + column as i64, py);
                blend(pixel, color, u16::from(alpha) * u16::from(c) / 255);
            }
        } else {
            blend_span(pixels, coverage, color);
        }
    }
}

/// Fills an axis-aligned device rectangle by its exact per-pixel area. Each
/// row is its left edge pixel, its interior at the row's vertical coverage
/// and its right edge pixel; an opaque colour over a fully covered interior
/// is stored as one fill.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "rectangle boundaries are clamped to the page and coverage to a byte"
)]
pub(super) fn fill_rectangle(
    target: &mut Target,
    bounds: [f64; 4],
    page: (u32, u32),
    color: Rgba8,
) {
    let [x0, y0, x1, y1] = bounds;
    let x0 = x0.clamp(0.0, f64::from(page.0));
    let x1 = x1.clamp(0.0, f64::from(page.0));
    let y0 = y0.clamp(0.0, f64::from(page.1));
    let y1 = y1.clamp(0.0, f64::from(page.1));
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let lo = x0.floor() as i64;
    let hi = x1.ceil() as i64;
    // An opaque fill in the background colour leaves background pixels as
    // they are at every coverage: c * a + c * (255 - a) rounds back to c.
    let background = color.a == 255 && color == target.background;
    for y in y0.floor() as i64..y1.ceil() as i64 {
        if background && target.unpainted(y, lo, hi) {
            continue;
        }
        let vertical = (y1.min(y as f64 + 1.0) - y0.max(y as f64)).clamp(0.0, 1.0);
        let area = |width: f64| (width * vertical * 255.0).round() as u16;
        if hi - lo == 1 {
            blend_run(target, y, (lo, hi), color, area(x1 - x0));
            continue;
        }
        blend_run(target, y, (lo, lo + 1), color, area(lo as f64 + 1.0 - x0));
        let coverage = area(1.0);
        if coverage == 255 && color.a == 255 {
            if let Some((_, interior)) = target.span(y, lo + 1, hi - 1) {
                interior
                    .as_chunks_mut::<4>()
                    .0
                    .fill([color.r, color.g, color.b, 255]);
            }
        } else {
            blend_run(target, y, (lo + 1, hi - 1), color, coverage);
        }
        blend_run(target, y, (hi - 1, hi), color, area(x1 - (hi - 1) as f64));
    }
}

/// Blends `color` at one coverage over the pixels of page row `y` from
/// `x0` (inclusive) to `x1` (exclusive) that lie in the tile.
fn blend_run(target: &mut Target, y: i64, (x0, x1): (i64, i64), color: Rgba8, alpha: u16) {
    if let Some((_, pixels)) = target.span(y, x0, x1) {
        for pixel in pixels.as_chunks_mut::<4>().0 {
            blend(pixel, color, alpha);
        }
    }
}

pub(super) fn blend_span(pixels: &mut [u8], coverage: &[u8], color: Rgba8) {
    let coverage = &coverage[..coverage.len().min(pixels.len() / 4)];
    if color == Rgba8::BLACK {
        black_span(pixels, coverage);
        return;
    }
    // An opaque colour at coverage 0 keeps the pixel and at 255 stores the
    // colour (c * 255 + p * 0 and c * 0 + p * 255 both round exactly), so
    // one fixed-width blend over the span stores what runs would.
    if color.a == 255 {
        partial_span(&mut pixels[..coverage.len() * 4], coverage, color);
        return;
    }
    let mut start = 0;
    while start < coverage.len() {
        if coverage[start] == 0 {
            start += 1;
            continue;
        }
        let full = coverage[start] == 255;
        let mut end = start + 1;
        while end < coverage.len() && coverage[end] != 0 && (coverage[end] == 255) == full {
            end += 1;
        }
        let pixels = &mut pixels[start * 4..end * 4];
        if full && color.a == 255 {
            pixels
                .as_chunks_mut::<4>()
                .0
                .fill([color.r, color.g, color.b, 255]);
        } else {
            partial_span(pixels, &coverage[start..end], color);
        }
        start = end;
    }
}

/// Opaque black: each colour channel keeps the complement of its coverage,
/// and alpha, opaque on every page this path renders, is kept. Fixed-width
/// weights expose four pixels' independent products to every target's
/// vectorizer.
fn black_span(pixels: &mut [u8], coverage: &[u8]) {
    let (blocks, tail) = pixels[..coverage.len() * 4].as_chunks_mut::<16>();
    let (alphas, remainder) = coverage.as_chunks::<4>();
    for (output, a) in blocks.iter_mut().zip(alphas) {
        let [a, b, c, d] = a.map(|v| 255 - u16::from(v));
        let weights = [a, a, a, 255, b, b, b, 255, c, c, c, 255, d, d, d, 255];
        for (out, weight) in output.iter_mut().zip(weights) {
            *out = div255(u16::from(*out) * weight);
        }
    }
    scalar_span(tail, remainder, Rgba8::BLACK);
}

#[inline]
fn partial_span(pixels: &mut [u8], coverage: &[u8], color: Rgba8) {
    if color.a != 255 || coverage.len() < 4 {
        scalar_span(pixels, coverage, color);
        return;
    }
    let source = [
        u16::from(color.r),
        u16::from(color.g),
        u16::from(color.b),
        255,
    ];
    let sources = [source; 4];
    let (blocks, tail) = pixels.as_chunks_mut::<16>();
    let (alphas, remainder) = coverage.as_chunks::<4>();
    for (output, a) in blocks.iter_mut().zip(alphas) {
        let values = [
            [u16::from(a[0]); 4],
            [u16::from(a[1]); 4],
            [u16::from(a[2]); 4],
            [u16::from(a[3]); 4],
        ];
        for ((out, &source), &alpha) in output
            .iter_mut()
            .zip(sources.as_flattened())
            .zip(values.as_flattened())
        {
            // The complementary weights sum to 255, bounding the sum by 65025.
            let value = source
                .wrapping_mul(alpha)
                .wrapping_add(u16::from(*out).wrapping_mul(255 - alpha));
            *out = div255(value);
        }
    }
    scalar_span(tail, remainder, color);
}

fn scalar_span(pixels: &mut [u8], coverage: &[u8], color: Rgba8) {
    if color == Rgba8::BLACK {
        for (pixel, &a) in pixels.as_chunks_mut::<4>().0.iter_mut().zip(coverage) {
            let inverse = 255 - u16::from(a);
            for channel in &mut pixel[..3] {
                *channel = div255(u16::from(*channel) * inverse);
            }
            pixel[3] = div255(255 * u16::from(a) + u16::from(pixel[3]) * inverse);
        }
    } else {
        for (p, &a) in pixels.as_chunks_mut::<4>().0.iter_mut().zip(coverage) {
            blend(p, color, u16::from(a));
        }
    }
}

/// Rounded division of a convex byte combination (at most 255 squared) by 255.
fn div255(value: u16) -> u8 {
    let shifted = value.wrapping_add(128);
    u8::try_from(shifted.wrapping_add(shifted >> 8) >> 8).unwrap_or(255)
}

/// Source-over of `color` at coverage `alpha` (0 to 255).
pub(super) fn blend(pixel: &mut [u8], color: Rgba8, alpha: u16) {
    let alpha = alpha * u16::from(color.a) / 255;
    for (out, source) in pixel.iter_mut().zip([color.r, color.g, color.b, 255]) {
        let value = u16::from(source)
            .wrapping_mul(alpha)
            .wrapping_add(u16::from(*out).wrapping_mul(255 - alpha));
        *out = div255(value);
    }
}

/// Premultiplied source-over of a rendered tile of the page.
#[expect(clippy::cast_possible_wrap, reason = "tile coordinates fit i64")]
pub(super) fn over(target: &mut Target, source: &Pixmap, tile: Tile) {
    let width = tile.width as usize * 4;
    for (row, source) in source.data().chunks_exact(width).enumerate() {
        let y = i64::from(tile.y) + row as i64;
        let x = i64::from(tile.x);
        let Some((start, pixels)) = target.span(y, x, x + i64::from(tile.width)) else {
            continue;
        };
        let skip = usize::try_from(start - x).unwrap_or(0) * 4;
        for (target, source) in pixels
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(source[skip..].as_chunks::<4>().0)
        {
            for (t, &s) in target.iter_mut().zip(source) {
                *t = u8::try_from(
                    u16::from(s) + (u16::from(*t) * u16::from(255 - source[3]) + 127) / 255,
                )
                .unwrap_or(255);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(width: u32, height: u32) -> Target {
        Target::new(
            Tile {
                x: 0,
                y: 0,
                width,
                height,
            },
            Rgba8::WHITE,
            pagura_core::Limits::default(),
        )
        .unwrap()
    }

    #[test]
    fn byte_division_matches_independent_integer_rounding() {
        for value in 0..=65025 {
            assert_eq!(u16::from(div255(value)), (value + 127) / 255);
        }
    }

    #[test]
    fn black_lanes_match_independent_integer_source_over() {
        for backdrop in 0..=255_u16 {
            for alpha in 0..=255_u16 {
                let mut pixels = [u8::try_from(backdrop).unwrap(); 28];
                for p in pixels.as_chunks_mut::<4>().0 {
                    p[3] = 255;
                }
                blend_span(
                    &mut pixels,
                    &[u8::try_from(alpha).unwrap(); 7],
                    Rgba8::BLACK,
                );
                let expected = u8::try_from((backdrop * (255 - alpha) + 127) / 255).unwrap();
                for p in pixels.as_chunks::<4>().0 {
                    assert_eq!(*p, [expected, expected, expected, 255]);
                }
            }
        }
    }

    #[test]
    fn fractional_rectangle_has_the_analytic_coverage() {
        let mut target = target(2, 2);
        fill_rectangle(&mut target, [0.25, 0.25, 1.25, 1.25], (2, 2), Rgba8::BLACK);
        let values: Vec<_> = target
            .finish()
            .data()
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| p[0])
            .collect();
        assert_eq!(values, [112, 207, 207, 239]);
    }

    #[test]
    fn span_kernels_preserve_one_rounding_per_store() {
        for color in [Rgba8::BLACK, Rgba8::WHITE, Rgba8::opaque(13, 91, 183)] {
            let coverage = [0, 1, 17, 128, 254, 255, 0, 255];
            let mut pixels = [57_u8; 32];
            for p in pixels.as_chunks_mut::<4>().0 {
                p[3] = 255;
            }
            blend_span(&mut pixels, &coverage, color);
            for (p, &a) in pixels.as_chunks::<4>().0.iter().zip(&coverage) {
                for (index, (&actual, source)) in
                    p.iter().zip([color.r, color.g, color.b, 255]).enumerate()
                {
                    let backdrop = if index == 3 { 255 } else { 57 };
                    let expected =
                        (u32::from(source) * u32::from(a) + backdrop * u32::from(255 - a) + 127)
                            / 255;
                    assert_eq!(u32::from(actual), expected);
                }
            }
        }
    }

    #[test]
    fn background_rectangles_skip_only_unpainted_rows() {
        let colors = [Rgba8::WHITE, Rgba8::opaque(13, 91, 183)];
        let mut skipped = target(6, 4);
        let mut written = target(6, 4);
        written.painted.iter_mut().for_each(|row| *row = [0, 6]);
        for target in [&mut skipped, &mut written] {
            fill_rectangle(target, [0.5, 1.25, 2.5, 2.75], (6, 4), colors[1]);
            fill_rectangle(target, [0.25, 0.5, 5.5, 3.5], (6, 4), colors[0]);
            fill_rectangle(target, [3.25, 0.0, 5.0, 4.0], (6, 4), colors[1]);
            fill_rectangle(target, [0.0, 0.25, 6.0, 1.5], (6, 4), colors[0]);
        }
        assert_eq!(skipped.finish().data(), written.finish().data());
    }

    #[test]
    fn tiles_receive_exactly_their_part_of_a_mark() {
        let mask = Coverage {
            x: 1,
            y: 1,
            width: 3,
            data: vec![10, 128, 255, 40, 200, 255],
        };
        let mut page = target(5, 4);
        blit(&mut page, &mask, (0, 0), Rgba8::opaque(20, 40, 60), None);
        let page = page.finish();
        let mut tile = Target::new(
            Tile {
                x: 2,
                y: 2,
                width: 3,
                height: 2,
            },
            Rgba8::WHITE,
            pagura_core::Limits::default(),
        )
        .unwrap();
        blit(&mut tile, &mask, (0, 0), Rgba8::opaque(20, 40, 60), None);
        let tile = tile.finish();
        for (row, pixels) in tile.data().as_chunks::<12>().0.iter().enumerate() {
            let start = ((row + 2) * 5 + 2) * 4;
            assert_eq!(pixels, &page.data()[start..start + 12]);
        }
    }
}
