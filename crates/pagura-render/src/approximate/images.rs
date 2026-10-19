//! Axis-aligned images from four equal-area midpoint samples per device
//! pixel, over the pixel's intersection with the image. Premultiplied sums
//! round once when stored. This approximates the image integration of
//! 8.9.5.3; other images take the exact compositor. Byte images take
//! [`bytes`], with the same values.

mod bytes;

use super::{clip::ClipMask, paint::Target};
use crate::{Tile, display::ImageCommand, interpreter::images::Image};
use pagura_core::{Error, Limits, Result};
use pagura_image::{ImageKind, MaskKind};
use pagura_raster::{Point, Rgba8, Transform};
use std::collections::HashMap;

/// Samples per device pixel along each axis.
const SAMPLES: usize = 2;

/// Paints `command` within `bounds`, the image's device pixels in the tile.
/// Returns `false`, having painted nothing, for an image that needs the exact
/// compositor: a rotated or sheared one, a transfer, matte or embedded mask.
pub(super) fn paint(
    command: &ImageCommand,
    device: Transform,
    target: &mut Target,
    bounds: Tile,
    clip: Option<&ClipMask>,
    limits: Limits,
) -> Result<bool> {
    if !eligible(command, device) {
        return Ok(false);
    }
    let m = command.transform.then(device);
    let Some(inverse) = m.inverse() else {
        return Ok(true);
    };
    let image_bounds = [
        m.e.min(m.e + m.a),
        m.f.min(m.f + m.d),
        m.e.max(m.e + m.a),
        m.f.max(m.f + m.d),
    ];
    // A clip that covers every pixel painted changes none of them.
    let clip = clip.filter(|c| {
        !c.covers(
            i64::from(bounds.x),
            i64::from(bounds.y),
            bounds.width as usize,
            bounds.height as usize,
        )
    });
    let mut source = Source::new(command)?;
    if bytes::paint(
        &source,
        target,
        bounds,
        (inverse, image_bounds),
        clip,
        limits,
    )? {
        return Ok(true);
    }
    paint_box(
        &mut source,
        target,
        bounds,
        inverse,
        clip,
        image_bounds,
        limits,
    )?;
    Ok(true)
}

/// Whether the box-sampled path handles `command`.
pub(super) fn eligible(command: &ImageCommand, device: Transform) -> bool {
    let m = command.transform.then(device);
    command.transfer.is_none()
        && !matches!(
            command.image.decoded.mask,
            MaskKind::Embedded(_) | MaskKind::InData
        )
        && command.image.matte.is_none()
        && m.b == 0.0
        && m.c == 0.0
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "sample counts are the fixed 2x2 grid and pixel bounds fit u32"
)]
fn paint_box(
    source: &mut Source<'_>,
    target: &mut Target,
    bounds: Tile,
    inverse: Transform,
    clip: Option<&ClipMask>,
    image_bounds: [f64; 4],
    limits: Limits,
) -> Result<()> {
    let width = bounds.width as usize;
    let height = bounds.height as usize;
    width
        .checked_mul(height)
        .and_then(|n| n.checked_mul(SAMPLES * SAMPLES))
        .and_then(|n| n.checked_add(width.checked_add(height)?.checked_mul(SAMPLES)?))
        .filter(|&n| n <= limits.raster_work)
        .ok_or(Error::limit("image sample work budget exceeded"))?;
    width
        .checked_mul(size_of::<Column>())
        .filter(|&n| n <= limits.decoded_bytes)
        .ok_or(Error::limit("image coordinate allocation budget exceeded"))?;
    let mut columns = Vec::new();
    columns
        .try_reserve_exact(width)
        .map_err(|_| Error::limit("image coordinate allocation failed"))?;
    for x in bounds.x..bounds.x + bounds.width {
        let x0 = f64::from(x).max(image_bounds[0]);
        let x1 = (f64::from(x) + 1.0).min(image_bounds[2]);
        let mut samples = [None; SAMPLES];
        for (sx, sample) in samples.iter_mut().enumerate() {
            let p = Point::new(x0 + (sx as f64 + 0.5) * (x1 - x0) / SAMPLES as f64, 0.0) * inverse;
            *sample = source.column(p.x)?;
        }
        columns.push(Column {
            samples,
            extent: (x1 - x0).max(0.0),
        });
    }
    for y in bounds.y..bounds.y + bounds.height {
        let y0 = f64::from(y).max(image_bounds[1]);
        let y1 = (f64::from(y) + 1.0).min(image_bounds[3]);
        let mut samples = [None; SAMPLES];
        for (sy, sample) in samples.iter_mut().enumerate() {
            let p = Point::new(0.0, y0 + (sy as f64 + 0.5) * (y1 - y0) / SAMPLES as f64) * inverse;
            *sample = source.row(p.y)?;
        }
        let mut colors = Vec::new();
        for (column, prepared) in columns.iter().enumerate() {
            let x = bounds.x + column as u32;
            let shape = ((prepared.extent * (y1 - y0).max(0.0)) * 255.0)
                .round()
                .clamp(0.0, 255.0) as u16;
            let coverage = clip.map_or(255, |c| c.at(i64::from(x), i64::from(y)));
            let coverage = u8::try_from(shape * u16::from(coverage) / 255).unwrap_or(255);
            if coverage == 0 {
                colors.push(None);
                continue;
            }
            let mut sum = [0_u32; 4];
            for &sy in &samples {
                for &sx in &prepared.samples {
                    let color = source.sample_grid(sx, sy)?;
                    let alpha = u32::from(color.a);
                    for (out, value) in sum.iter_mut().zip([color.r, color.g, color.b, 255]) {
                        *out += u32::from(value) * alpha;
                    }
                }
            }
            colors.push(Some((sum, coverage)));
        }
        target.paint_row(i64::from(y), i64::from(bounds.x), width, |column, pixel| {
            if let Some(Some((sum, coverage))) = colors.get(column) {
                composite(pixel, *sum, (SAMPLES * SAMPLES) as u32, *coverage);
            }
        });
    }
    Ok(())
}

struct Column {
    samples: [Option<Coordinate>; SAMPLES],
    extent: f64,
}

fn composite(pixel: &mut [u8; 4], sum: [u32; 4], count: u32, coverage: u8) {
    let denominator = count * 65025;
    let coverage = u32::from(coverage);
    let inverse = denominator - sum[3] / 255 * coverage;
    for (out, value) in pixel[..3].iter_mut().zip(sum) {
        let value = value * coverage + u32::from(*out) * inverse;
        *out = u8::try_from((value + denominator / 2) / denominator).unwrap_or(255);
    }
    let alpha = sum[3] * coverage + u32::from(pixel[3]) * inverse;
    pixel[3] = u8::try_from((alpha + denominator / 2) / denominator).unwrap_or(255);
}

struct Source<'a> {
    command: &'a ImageCommand,
    quick: Quick<'a>,
    stencil: Rgba8,
    palette: HashMap<u16, Rgba8>,
}

impl<'a> Source<'a> {
    fn new(command: &'a ImageCommand) -> Result<Self> {
        Ok(Self {
            command,
            quick: Quick::new(&command.image, command.composite.conversion.intent)?,
            stencil: command
                .stencil
                .to_rgba(None, command.composite.conversion.intent)?,
            palette: HashMap::new(),
        })
    }

    fn column(&self, value: f64) -> Result<Option<Coordinate>> {
        coordinate(
            value,
            (
                self.command.image.decoded.width,
                self.command.image.decoded.extent().0,
            ),
            self.command.image.mask.as_ref().map(|m| m.decoded.width),
            false,
        )
    }

    fn row(&self, value: f64) -> Result<Option<Coordinate>> {
        coordinate(
            value,
            (
                self.command.image.decoded.height,
                self.command.image.decoded.extent().1,
            ),
            self.command.image.mask.as_ref().map(|m| m.decoded.height),
            true,
        )
    }

    fn sample_grid(&mut self, x: Option<Coordinate>, y: Option<Coordinate>) -> Result<Rgba8> {
        let (Some(x), Some(y)) = (x, y) else {
            return Ok(Rgba8::TRANSPARENT);
        };
        let image = &self.command.image;
        let index = y.color * image.decoded.width as usize + x.color;
        let mask = image
            .mask
            .as_ref()
            .map(|m| y.mask * m.decoded.width as usize + x.mask);
        let Some(mut color) =
            self.quick
                .sample(self.command, index, self.stencil, &mut self.palette)?
        else {
            return Ok(Rgba8::TRANSPARENT);
        };
        if let Some(mask) = mask {
            let image = self
                .command
                .image
                .mask
                .as_ref()
                .ok_or(Error::syntax("missing image mask"))?;
            color.a = mask_alpha(image, mask)?;
        }
        Ok(color)
    }
}

/// Direct lookups for 8-bit RGB and palettes of one-component images.
enum Quick<'a> {
    Rgb(&'a [u8]),
    One(&'a [u8], Box<[Rgba8; 256]>),
    General,
}

impl<'a> Quick<'a> {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "palette components are clamped to byte range"
    )]
    fn new(image: &'a Image, intent: crate::icc::Intent) -> Result<Self> {
        let pagura_image::Samples::Bytes(samples) = &image.decoded.samples else {
            return Ok(Self::General);
        };
        if image.decoded.components == 3
            && image.decoded.bits_per_component == 8
            && matches!(image.space, Some(crate::color::Space::Rgb))
            && image.decode == [(0.0, 1.0); 3]
        {
            return Ok(Self::Rgb(samples));
        }
        if image.decoded.components == 1
            && (1..=8).contains(&image.decoded.bits_per_component)
            && let Some(space) = &image.space
        {
            let mut table = Box::new([Rgba8::BLACK; 256]);
            let maximum = (1_u16 << image.decoded.bits_per_component) - 1;
            let (low, high) = image.decode[0];
            for value in 0..=maximum {
                let component = low + f64::from(value) / f64::from(maximum) * (high - low);
                let [r, g, b] = space
                    .convert_with_intent(&[component], intent)?
                    .map(|c| (c * 255.0).round().clamp(0.0, 255.0) as u8);
                table[usize::from(value)] = Rgba8::opaque(r, g, b);
            }
            return Ok(Self::One(samples, table));
        }
        Ok(Self::General)
    }

    fn sample(
        &self,
        command: &ImageCommand,
        index: usize,
        stencil: Rgba8,
        palette: &mut HashMap<u16, Rgba8>,
    ) -> Result<Option<Rgba8>> {
        if !matches!(
            command.image.decoded.mask,
            MaskKind::None | MaskKind::Soft(_) | MaskKind::Stencil(_)
        ) {
            return texel(command, index, stencil, palette);
        }
        let color = match self {
            Self::Rgb(bytes) => {
                let values = bytes
                    .get(index * 3..index * 3 + 3)
                    .ok_or(Error::syntax("missing RGB sample"))?;
                Rgba8::opaque(values[0], values[1], values[2])
            }
            Self::One(bytes, table) => {
                table[usize::from(
                    *bytes
                        .get(index)
                        .ok_or(Error::syntax("missing colour sample"))?,
                )]
            }
            Self::General => return texel(command, index, stencil, palette),
        };
        Ok(Some(color))
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "channels clamped to byte range"
)]
fn texel(
    command: &ImageCommand,
    index: usize,
    stencil: Rgba8,
    palette: &mut HashMap<u16, Rgba8>,
) -> Result<Option<Rgba8>> {
    let image = &command.image;
    let color = if let ImageKind::Stencil { inverted } = image.decoded.kind {
        let sample = image
            .decoded
            .samples
            .get(index)
            .ok_or(Error::syntax("missing stencil sample"))?;
        if (sample == 1) != inverted {
            return Ok(None);
        }
        stencil
    } else {
        let n = usize::from(image.decoded.components);
        let sample = image
            .decoded
            .samples
            .get(index * n)
            .ok_or(Error::syntax("missing image sample"))?;
        let key = if n == 1 { Some(sample) } else { None };
        if let Some(color) = key.and_then(|k| palette.get(&k)).copied() {
            color
        } else {
            let mut values = [0.0; 32];
            image.components(index, &mut values)?;
            let space = image
                .space
                .as_ref()
                .ok_or(Error::syntax("missing image colour space"))?;
            let rgb =
                space.convert_with_intent(&values[..n], command.composite.conversion.intent)?;
            let [r, g, b] = rgb.map(|c| (c * 255.0).round().clamp(0.0, 255.0) as u8);
            let color = Rgba8::opaque(r, g, b);
            if let Some(key) = key {
                palette.insert(key, color);
            }
            color
        }
    };
    if let MaskKind::ColorKey(ranges) = &image.decoded.mask {
        let n = usize::from(image.decoded.components);
        if ranges.iter().enumerate().all(|(c, &(low, high))| {
            image
                .decoded
                .samples
                .get(index * n + c)
                .is_some_and(|s| s >= low && s <= high)
        }) {
            return Ok(None);
        }
    }
    Ok(Some(color))
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "mask values are clamped to byte range"
)]
fn mask_alpha(image: &Image, index: usize) -> Result<u8> {
    if let ImageKind::Stencil { inverted } = image.decoded.kind {
        let value = image
            .decoded
            .samples
            .get(index)
            .ok_or(Error::syntax("missing image mask sample"))?;
        return Ok(if (value == 1) == inverted { 255 } else { 0 });
    }
    if image.decoded.bits_per_component == 8 && image.decode == [(0.0, 1.0)] {
        return image
            .decoded
            .samples
            .get(index)
            .and_then(|v| u8::try_from(v).ok())
            .ok_or(Error::syntax("missing mask sample"));
    }
    let mut values = [0.0; 32];
    image.components(index, &mut values)?;
    Ok((values[0] * 255.0).round().clamp(0.0, 255.0) as u8)
}

#[derive(Clone, Copy)]
struct Coordinate {
    color: usize,
    mask: usize,
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "normalized finite coordinates and decoded dimensions bound sample indices"
)]
fn coordinate(
    value: f64,
    (length, extent): (u32, f64),
    mask: Option<u32>,
    vertical: bool,
) -> Result<Option<Coordinate>> {
    if !value.is_finite() {
        return Err(Error::limit("image sampling coordinate overflow"));
    }
    if !(0.0..1.0).contains(&value) {
        return Ok(None);
    }
    if length == 0 || mask == Some(0) {
        return Err(Error::syntax("empty image sample axis"));
    }
    let p = if vertical { 1.0 - value } else { value };
    let color = ((p * extent) as usize).min(length as usize - 1);
    let mask = mask.map_or(0, |length| {
        ((p * f64::from(length)).floor() as usize).min(length as usize - 1)
    });
    Ok(Some(Coordinate { color, mask }))
}

#[cfg(test)]
mod tests {
    use super::{composite, coordinate};

    #[test]
    fn nearest_indices_follow_independent_half_open_cells() {
        for (p, forward, reverse, mask) in [
            (0.0, 0, 7, 0),
            (0.125, 1, 7, 0),
            (0.5, 4, 4, 1),
            (0.875, 7, 1, 1),
        ] {
            let c = coordinate(p, (8, 8.0), Some(2), false).unwrap().unwrap();
            assert_eq!((c.color, c.mask), (forward, mask));
            assert_eq!(
                coordinate(p, (8, 8.0), None, true).unwrap().unwrap().color,
                reverse
            );
        }
        assert!(coordinate(1.0, (8, 8.0), None, false).unwrap().is_none());
        assert!(coordinate(-0.1, (8, 8.0), None, false).unwrap().is_none());
        assert!(coordinate(f64::NAN, (8, 8.0), None, false).is_err());
        assert!(coordinate(0.0, (0, 0.0), None, false).is_err());
    }

    #[test]
    fn partial_samples_match_independent_rational_source_over() {
        // Two opaque red samples, one half-opacity green, one transparent.
        let sum = [2 * 255 * 255, 255 * 128, 0, (2 * 255 + 128) * 255];
        for clip in [0, 1, 127, 255] {
            let mut pixel = [17, 31, 53, 255];
            composite(&mut pixel, sum, 4, clip);
            for (i, backdrop) in [17_u64, 31, 53].into_iter().enumerate() {
                let d = 4 * 255 * 255_u64;
                let a = u64::from(clip) * (2 * 255 + 128);
                let s = u64::from(clip) * u64::from(sum[i]);
                assert_eq!(u64::from(pixel[i]), (s + backdrop * (d - a) + d / 2) / d);
            }
            assert_eq!(pixel[3], 255);
        }
    }
}
