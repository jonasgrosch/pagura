//! Byte images through separable sample indices. Each device column's and
//! row's sample positions are resolved to image and mask indices once per
//! image, so a pixel only reads its samples. Values match the general
//! sampler's.

use super::{Quick, SAMPLES, Source, composite};
use crate::Tile;
use crate::approximate::{clip::ClipMask, paint::Target};
use crate::interpreter::images::Image;
use pagura_core::{Error, Limits, Result};
use pagura_image::{ImageKind, MaskKind, Samples};
use pagura_raster::Transform;

/// One sample position on one axis.
#[derive(Clone, Copy, Default)]
struct Index {
    image: usize,
    mask: usize,
    inside: bool,
}

/// The sample positions of one device column or row, the length of its
/// intersection with the image, and its rectangle-clip overlap.
#[derive(Clone, Copy)]
struct Axis {
    samples: [Index; SAMPLES],
    span: f64,
    clip: f64,
}

/// An image's byte samples: `color` reads one colour by sample index, and a
/// byte mask has its own grid.
struct Bytes<'a, F> {
    color: F,
    /// Stored samples per row and column.
    size: (u32, u32),
    /// The extent of the unit square in stored samples
    /// ([`pagura_image::DecodedImage::extent`]).
    extent: (f64, f64),
    mask: Option<(&'a [u8], (u32, u32))>,
}

/// Paints `source` within `bounds`, or returns `false`, having painted
/// nothing, when its samples or mask need the general sampler or its column
/// indices exceed a quarter of the allocation budget.
pub(super) fn paint(
    source: &Source<'_>,
    target: &mut Target,
    bounds: Tile,
    placement: (Transform, [f64; 4]),
    clip: Option<&ClipMask>,
    limits: Limits,
) -> Result<bool> {
    let image = &source.command.image;
    if !matches!(
        image.decoded.mask,
        MaskKind::None | MaskKind::Soft(_) | MaskKind::Stencil(_)
    ) {
        return Ok(false);
    }
    let mask = match image.mask.as_deref() {
        None => None,
        Some(mask) => match byte_mask(mask) {
            Some(bytes) => Some((bytes, (mask.decoded.width, mask.decoded.height))),
            None => return Ok(false),
        },
    };
    let size = (image.decoded.width, image.decoded.height);
    let extent = image.decoded.extent();
    match &source.quick {
        Quick::Rgb(bytes) => {
            let color = |index: usize| {
                let rgb = bytes
                    .get(index * 3..index * 3 + 3)
                    .ok_or(Error::syntax("missing RGB sample"))?;
                Ok([rgb[0], rgb[1], rgb[2]])
            };
            let bytes = Bytes {
                color,
                size,
                extent,
                mask,
            };
            dispatch(&bytes, target, bounds, placement, clip, limits)
        }
        Quick::One(bytes, table) => {
            let color = |index: usize| {
                let value = bytes
                    .get(index)
                    .ok_or(Error::syntax("missing colour sample"))?;
                let c = table[usize::from(*value)];
                Ok([c.r, c.g, c.b])
            };
            let bytes = Bytes {
                color,
                size,
                extent,
                mask,
            };
            dispatch(&bytes, target, bounds, placement, clip, limits)
        }
        Quick::General => Ok(false),
    }
}

/// The samples of an 8-bit mask whose bytes are its alpha.
fn byte_mask(image: &Image) -> Option<&[u8]> {
    match &image.decoded.samples {
        Samples::Bytes(bytes)
            if !matches!(image.decoded.kind, ImageKind::Stencil { .. })
                && image.decoded.bits_per_component == 8
                && image.decode == [(0.0, 1.0)] =>
        {
            Some(bytes)
        }
        _ => None,
    }
}

fn dispatch<F: Fn(usize) -> Result<[u8; 3]>>(
    bytes: &Bytes<'_, F>,
    target: &mut Target,
    bounds: Tile,
    placement: (Transform, [f64; 4]),
    clip: Option<&ClipMask>,
    limits: Limits,
) -> Result<bool> {
    if bytes.mask.is_none() {
        render::<F, true>(bytes, target, bounds, placement, clip, limits)
    } else {
        render::<F, false>(bytes, target, bounds, placement, clip, limits)
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "shape coverage is clamped to a byte and offsets lie in the checked bounds"
)]
fn render<F: Fn(usize) -> Result<[u8; 3]>, const OPAQUE: bool>(
    bytes: &Bytes<'_, F>,
    target: &mut Target,
    bounds: Tile,
    (inverse, image_bounds): (Transform, [f64; 4]),
    clip: Option<&ClipMask>,
    limits: Limits,
) -> Result<bool> {
    let maximum = limits.decoded_bytes / 4 / size_of::<Axis>();
    let width = bounds.width as usize;
    if width > maximum {
        return Ok(false);
    }
    u64::from(bounds.width)
        .checked_mul(u64::from(bounds.height))
        .and_then(|n| n.checked_mul((SAMPLES * SAMPLES) as u64))
        .and_then(|n| {
            n.checked_add((u64::from(bounds.width) + u64::from(bounds.height)) * SAMPLES as u64)
        })
        .filter(|&n| n <= limits.raster_work as u64)
        .ok_or(Error::limit("image sample work budget exceeded"))?;
    let rectangle = match clip {
        Some(ClipMask::Rectangle(r)) => Some(*r),
        _ => None,
    };
    let (mask_width, mask_height) = bytes.mask.map_or((1, 1), |(_, size)| size);
    let mut columns = Vec::new();
    columns
        .try_reserve_exact(width)
        .map_err(|_| Error::limit("image index allocation failed"))?;
    for x in bounds.x..bounds.x + bounds.width {
        let mut column = axis(
            x,
            (image_bounds[0], image_bounds[2]),
            (inverse.a, inverse.e),
            ((bytes.size.0, bytes.extent.0), mask_width),
            false,
        )?;
        column.clip = rectangle.map_or(1.0, |r| overlap(x, r[0], r[2]));
        columns.push(column);
    }
    for y in bounds.y..bounds.y + bounds.height {
        let row = axis(
            y,
            (image_bounds[1], image_bounds[3]),
            (inverse.d, inverse.f),
            ((bytes.size.1, bytes.extent.1), mask_height),
            true,
        )?;
        let clip_y = rectangle.map_or(1.0, |r| overlap(y, r[1], r[3]));
        let Some((start, pixels)) = target.span(
            i64::from(y),
            i64::from(bounds.x),
            i64::from(bounds.x) + i64::from(bounds.width),
        ) else {
            continue;
        };
        for (pixel, (column, x)) in pixels
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(columns.iter().zip(start..))
        {
            let shape = if column.span == 1.0 && row.span == 1.0 {
                255
            } else {
                (column.span * row.span * 255.0).round().clamp(0.0, 255.0) as u16
            };
            // As `ClipMask::at`, separated into its column and row overlaps.
            let clipped = match (rectangle, clip) {
                (Some(_), _) if column.clip == 1.0 && clip_y == 1.0 => 255,
                (Some(_), _) => (column.clip * clip_y * 255.0).round() as u8,
                (None, Some(c)) => c.at(x, i64::from(y)),
                (None, None) => 255,
            };
            let coverage = u8::try_from(shape * u16::from(clipped) / 255).unwrap_or(255);
            sample::<F, OPAQUE>(bytes, column, &row, pixel, coverage)?;
        }
    }
    Ok(true)
}

/// Composites one pixel's samples at `coverage`. An opaque, fully covered
/// pixel is their rounded mean, which is what source-over gives it.
#[inline(always)]
#[expect(
    clippy::inline_always,
    clippy::cast_possible_truncation,
    reason = "keeps the colour reader and opacity specialized in the pixel loop; \
              sample counts are the fixed grid"
)]
fn sample<F: Fn(usize) -> Result<[u8; 3]>, const OPAQUE: bool>(
    bytes: &Bytes<'_, F>,
    column: &Axis,
    row: &Axis,
    pixel: &mut [u8; 4],
    coverage: u8,
) -> Result<()> {
    if coverage == 0 {
        return Ok(());
    }
    let width = bytes.size.0 as usize;
    let count = (SAMPLES * SAMPLES) as u32;
    if OPAQUE
        && coverage == 255
        && row.samples.iter().all(|s| s.inside)
        && column.samples.iter().all(|s| s.inside)
    {
        let mut sum = [0_u32; 3];
        for sy in row.samples {
            for sx in column.samples {
                let rgb = (bytes.color)(sy.image * width + sx.image)?;
                for (out, value) in sum.iter_mut().zip(rgb) {
                    *out += u32::from(value);
                }
            }
        }
        for (out, value) in pixel[..3].iter_mut().zip(sum) {
            *out = u8::try_from((value + count / 2) / count).unwrap_or(255);
        }
        return Ok(());
    }
    let mut sum = [0_u32; 4];
    for sy in row.samples {
        for sx in column.samples {
            if !sx.inside || !sy.inside {
                continue;
            }
            let alpha = match bytes.mask {
                Some((mask, (mask_width, _))) if !OPAQUE => u32::from(
                    *mask
                        .get(sy.mask * mask_width as usize + sx.mask)
                        .ok_or(Error::syntax("missing mask sample"))?,
                ),
                _ => 255,
            };
            // A transparent sample adds nothing, so its colour is not read.
            if alpha == 0 {
                continue;
            }
            let [r, g, b] = (bytes.color)(sy.image * width + sx.image)?;
            for (out, value) in sum.iter_mut().zip([r, g, b, 255]) {
                *out += u32::from(value) * alpha;
            }
        }
    }
    composite(pixel, sum, count, coverage);
    Ok(())
}

/// The sample positions of device column or row `pixel` within the image's
/// device extent `low..high`, mapped to image space by `scale` and
/// `translation`. Rows count from the image's top (`flip`).
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "positions are checked finite and clamped to the validated sample grids"
)]
fn axis(
    pixel: u32,
    (low, high): (f64, f64),
    (scale, translation): (f64, f64),
    ((length, extent), mask): ((u32, f64), u32),
    flip: bool,
) -> Result<Axis> {
    if length == 0 || mask == 0 {
        return Err(Error::syntax("empty image sample axis"));
    }
    let low = f64::from(pixel).max(low);
    let high = (f64::from(pixel) + 1.0).min(high);
    let mut samples = [Index::default(); SAMPLES];
    for (s, index) in samples.iter_mut().enumerate() {
        let value = (low + (s as f64 + 0.5) * (high - low) / SAMPLES as f64) * scale + translation;
        if !value.is_finite() {
            return Err(Error::limit("image sampling coordinate overflow"));
        }
        index.inside = (0.0..1.0).contains(&value);
        if index.inside {
            let p = if flip { 1.0 - value } else { value };
            index.image = ((p * extent) as usize).min(length as usize - 1);
            index.mask = ((p * f64::from(mask)) as usize).min(mask as usize - 1);
        }
    }
    Ok(Axis {
        samples,
        span: (high - low).max(0.0),
        clip: 1.0,
    })
}

fn overlap(pixel: u32, low: f64, high: f64) -> f64 {
    (high.min(f64::from(pixel) + 1.0) - low.max(f64::from(pixel))).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_and_mask_grids_have_independent_rational_indices() {
        // Device column 2 of an image eight columns wide samples it at 9/32
        // and 11/32 of its width.
        let x = axis(2, (0.0, 8.0), (0.125, 0.0), ((8, 8.0), 16), false).unwrap();
        assert_eq!(x.samples.map(|s| s.image), [2, 2]);
        assert_eq!(x.samples.map(|s| s.mask), [4, 5]);
        assert!(x.samples.iter().all(|s| s.inside));
        let y = axis(2, (0.0, 8.0), (0.125, 0.0), ((8, 8.0), 16), true).unwrap();
        assert_eq!(y.samples.map(|s| s.image), [5, 5]);
        assert_eq!(y.samples.map(|s| s.mask), [11, 10]);
        let outside = axis(2, (0.0, 8.0), (1.0, 0.0), ((8, 8.0), 16), false).unwrap();
        assert!(outside.samples.iter().all(|s| !s.inside));
        assert!(axis(2, (0.0, 8.0), (f64::INFINITY, 0.0), ((8, 8.0), 16), false).is_err());
        assert!(axis(2, (0.0, 8.0), (0.125, 0.0), ((0, 0.0), 16), false).is_err());
    }
}
