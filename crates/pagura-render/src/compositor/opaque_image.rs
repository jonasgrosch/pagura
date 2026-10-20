//! A sole opaque DeviceGray or DeviceRGB image can reach the output without a
//! group surface (11.3.3), using the general compositor's sampling geometry.
//! Magnified images take their nearest sample (8.9.5.3) or, with
//! `/Interpolate`, bilinear weights. Minified images placed upright or by a
//! quarter turn take the exact area average of their sample levels over the
//! separable footprints (ADR 0004): integer interior sums and f64 end
//! weights, rounded once. Coverage comes from the common rasterizer; a
//! partially covered pixel mixes its unrounded level with the opaque
//! background before that rounding.

use super::{
    Context, Model,
    images::sampler_for,
    reduced::Span,
    sampled::{Sampler, coordinate},
};
use crate::Tile;
use crate::display::ImageCommand;
use crate::interpreter::images::Image;
use pagura_core::{Error, Result};
use pagura_image::{MaskKind, Samples};
use pagura_raster::{FillRule, Mask, PathBuilder, Pixmap};

/// Longest run of byte codes whose integer sum fits in a `u32`.
const RUN: usize = RUN_LINES as usize;
/// [`RUN`] as a count of source lines.
const RUN_LINES: u32 = u32::MAX / 255;

impl Context {
    // Keep image preparation separate from the general tile compositor.
    #[inline(never)]
    pub(super) fn paint_opaque_image(
        &mut self,
        command: &ImageCommand,
        model: Model,
        output: &mut Pixmap,
        (tile, region): (Tile, Tile),
        background: [u8; 4],
    ) -> Result<bool> {
        self.paint_levels_of(command, model, output, (tile, region), (background, false))
    }

    /// As [`Self::paint_opaque_image`] for the colour samples alone of an
    /// image with a soft mask, whose mask is then sampled as an image of its
    /// own ([`crate::Accuracy::Competitive`]'s byte-page interiors).
    pub(super) fn paint_image_colour(
        &mut self,
        command: &ImageCommand,
        model: Model,
        output: &mut Pixmap,
        region: Tile,
    ) -> Result<bool> {
        self.paint_levels_of(command, model, output, (region, region), ([255; 4], true))
    }

    fn paint_levels_of(
        &mut self,
        command: &ImageCommand,
        model: Model,
        output: &mut Pixmap,
        (tile, region): (Tile, Tile),
        (background, colour_only): ([u8; 4], bool),
    ) -> Result<bool> {
        if model != Model::Rgb
            || self.separations.enabled
            || self.limits.depth == 0
            || !command.composite.simple()
            || command.composite.overprint
            || command.transfer.is_some()
            || command.clip.is_some()
        {
            return Ok(false);
        }
        let target = Target {
            output,
            tile,
            region,
            background,
        };
        match command.image.decoded.components {
            1 => self.paint_levels::<1>(command, target, colour_only),
            3 => self.paint_levels::<3>(command, target, colour_only),
            _ => Ok(false),
        }
    }

    fn paint_levels<const N: usize>(
        &mut self,
        command: &ImageCommand,
        mut target: Target<'_>,
        colour_only: bool,
    ) -> Result<bool> {
        let Some(levels) = Levels::<N>::new(&command.image, colour_only) else {
            return Ok(false);
        };
        let transform = command.transform.then(self.device);
        let Some(inverse) = transform.inverse() else {
            return Ok(false);
        };
        let sampler = sampler_for(&command.image, inverse);
        if !sampler.inverse.is_finite() {
            return Ok(false);
        }
        let Some(footprint) = Footprint::new(&sampler) else {
            return Ok(false);
        };
        // Decline whenever the group path's page surface, or the charge for its
        // one command, would exceed a budget, so that path reports the error;
        // this path charges the same work: the surface, the mark, one clip
        // visit and one sample per pixel, plus any further footprint cells.
        let Ok((count, _)) = self.admit_surface(target.region) else {
            return Ok(false);
        };
        if self.work + count >= self.limits.raster_work {
            return Ok(false);
        }
        let clip = self
            .clip(None, target.region)
            .map_err(|error| crate::display::contextual(error, command.position))?;
        let mut bounds = PathBuilder::new();
        bounds.rect(0.0, 0.0, 1.0, 1.0);
        let Some(clip) = clip
            .clipped_path(&bounds.finish(), transform, FillRule::NonZero, self.limits)
            .map_err(|error| crate::display::contextual(error, command.position))?
        else {
            return Ok(false);
        };
        // Pixels outside the image, or partly inside it, show the background
        // that the output already holds; only an opaque one mixes simply, and
        // chunk boundaries must not decide which path a tile takes.
        if target.background[3] != 255 {
            return Ok(false);
        }
        let plan = Plan::new(footprint, &sampler, &clip)
            .map_err(|error| crate::display::contextual(error, command.position))?;
        let bytes = plan.bytes::<N>();
        if self
            .allocated
            .checked_add(bytes)
            .is_none_or(|n| n > self.limits.decoded_bytes)
        {
            return Ok(false);
        }
        self.charge(count)?;
        self.charge(1)?;
        self.trim_raster(bytes);
        self.allocated += bytes;
        let result = (|| {
            self.charge(count)?;
            self.charge(count)?;
            match &plan {
                Plan::Nearest(centres) => {
                    target.nearest(&clip, &sampler, centres.as_ref(), &levels)
                }
                Plan::Bilinear(centres) => self.bilinear(&mut target, &clip, centres, &levels),
                Plan::Area(area) => self.area(&mut target, &clip, area, &levels),
            }
        })();
        self.allocated -= bytes;
        result
            .map(|()| true)
            .map_err(|error| crate::display::contextual(error, command.position))
    }

    fn bilinear<const N: usize>(
        &mut self,
        target: &mut Target<'_>,
        clip: &Mask,
        centres: &Centres,
        levels: &Levels<'_, N>,
    ) -> Result<()> {
        let origin = clip.origin();
        for (row, column, coverage) in clip.spans() {
            self.charge(coverage.len().saturating_mul(3))?;
            let (x, y) = (origin.0 + index(column)?, origin.1 + index(row)?);
            let (row, columns) = centres.offsets(row, column, coverage.len())?;
            target.store(x, y, coverage, |pixel| {
                let column = columns
                    .get(pixel)
                    .ok_or(Error::invalid_argument("image run outside centres"))?;
                let sample = |offset: usize| {
                    levels
                        .samples
                        .get(offset)
                        .map(|sample| sample.map(f64::from))
                        .ok_or(Error::syntax("missing image sample"))
                };
                let mix = |a: [f64; N], b: [f64; N], t: f64| -> [f64; N] {
                    std::array::from_fn(|c| a[c] * (1.0 - t) + b[c] * t)
                };
                let low = mix(
                    sample(row.low + column.low)?,
                    sample(row.low + column.high)?,
                    column.fraction,
                );
                let high = mix(
                    sample(row.high + column.low)?,
                    sample(row.high + column.high)?,
                    column.fraction,
                );
                Ok(Some(levels.level(mix(low, high, row.fraction))))
            })?;
        }
        Ok(())
    }

    /// Exact area averages, streamed by device row. Upright images sum each
    /// source row across every column footprint, then weight the rows of the
    /// row footprint; quarter turns sum each source row across the device
    /// row's footprint, then weight the rows under each column.
    fn area<const N: usize>(
        &mut self,
        target: &mut Target<'_>,
        clip: &Mask,
        plan: &Area,
        levels: &Levels<'_, N>,
    ) -> Result<()> {
        let mut values = filled(plan.columns.len(), [0.0; N])?;
        let mut sums = filled(plan.lines(), [0.0; N])?;
        let mut codes = filled(if plan.swapped { 0 } else { plan.lines() }, [0; N])?;
        let mut current = None;
        let origin = clip.origin();
        for (row, column, coverage) in clip.spans() {
            let y = origin.1 + index(row)?;
            if current != Some(y) {
                let span = *plan
                    .rows
                    .get(row)
                    .ok_or(Error::invalid_argument("image row outside footprints"))?;
                if plan.swapped {
                    self.turned_row(levels, plan, span, &mut sums, &mut values)?;
                } else {
                    self.upright_row(levels, plan, span, (&mut sums, &mut codes), &mut values)?;
                }
                current = Some(y);
            }
            let (Some(values), Some(columns)) = (
                values.get(column..column + coverage.len()),
                plan.columns.get(column..column + coverage.len()),
            ) else {
                return Err(Error::invalid_argument("image run outside footprints"));
            };
            if plan.rows.get(row).is_some_and(Span::is_empty) {
                continue;
            }
            target.store(origin.0 + index(column)?, y, coverage, |pixel| {
                Ok(values
                    .get(pixel)
                    .zip(columns.get(pixel))
                    .filter(|(_, column)| !column.is_empty())
                    .map(|(&value, _)| levels.level(value)))
            })?;
        }
        Ok(())
    }

    /// The footprint means of one device row of an upright placement. The
    /// row's source lines are summed first, its end lines weighted around an
    /// exact integer sum of the lines between them, and each column's
    /// footprint then sums that line as [`line_sums`] sums a source line.
    fn upright_row<const N: usize>(
        &mut self,
        levels: &Levels<'_, N>,
        plan: &Area,
        span: Span,
        (line, codes): (&mut [[f64; N]], &mut [[u32; N]]),
        values: &mut [[f64; N]],
    ) -> Result<()> {
        let (first, end) = (plan.first as usize, plan.end as usize);
        let cells = |row: u32| -> Result<&[[u8; N]]> {
            levels
                .row(row)?
                .get(first..end)
                .ok_or(Error::syntax("image footprint outside samples"))
        };
        self.charge(line.len().saturating_mul((span.end - span.first) as usize))?;
        match span.end - span.first {
            0 => line.fill([0.0; N]),
            1 => {
                for (out, sample) in line.iter_mut().zip(cells(span.first)?) {
                    *out = sample.map(|v| span.first_weight * f64::from(v));
                }
            }
            _ => {
                for (out, (top, bottom)) in line
                    .iter_mut()
                    .zip(cells(span.first)?.iter().zip(cells(span.end - 1)?))
                {
                    *out = std::array::from_fn(|c| {
                        span.first_weight * f64::from(top[c])
                            + span.last_weight * f64::from(bottom[c])
                    });
                }
                let interior = span.first + 1..span.end - 1;
                for start in interior.clone().step_by(RUN) {
                    // A run's codes sum below 2^32, so wrapping additions are
                    // exact and, free of overflow checks, vectorize.
                    codes.fill([0; N]);
                    for row in start..interior.end.min(start.saturating_add(RUN_LINES)) {
                        let samples = cells(row)?.as_flattened();
                        for (code, &value) in codes.as_flattened_mut().iter_mut().zip(samples) {
                            *code = code.wrapping_add(u32::from(value));
                        }
                    }
                    for (out, &code) in line.as_flattened_mut().iter_mut().zip(codes.as_flattened())
                    {
                        *out += f64::from(code);
                    }
                }
            }
        }
        self.charge(plan.samples)?;
        for (value, column) in values.iter_mut().zip(&plan.columns) {
            let cells = line
                .get(column.first as usize - first..column.end as usize - first)
                .ok_or(Error::invalid_argument("image footprint outside line"))?;
            let total = match cells {
                [] => [0.0; N],
                [only] => only.map(|v| column.first_weight * v),
                [head, interior @ .., tail] => {
                    let mut total = [0.0; N];
                    for cell in interior {
                        for (total, &v) in total.iter_mut().zip(cell) {
                            *total += v;
                        }
                    }
                    std::array::from_fn(|c| {
                        column.first_weight * head[c] + total[c] + column.last_weight * tail[c]
                    })
                }
            };
            let normalization = column.normalization * span.normalization;
            *value = total.map(|v| v * normalization);
        }
        Ok(())
    }

    fn turned_row<const N: usize>(
        &mut self,
        levels: &Levels<'_, N>,
        plan: &Area,
        span: Span,
        sums: &mut [[f64; N]],
        values: &mut [[f64; N]],
    ) -> Result<()> {
        let cells = (span.end - span.first) as usize;
        self.charge(sums.len().saturating_mul(cells))?;
        for (row, sum) in (plan.first..).zip(sums.iter_mut()) {
            line_sums(
                levels.row(row)?,
                std::slice::from_ref(&span),
                std::slice::from_mut(sum),
            )?;
        }
        self.charge(plan.samples)?;
        for (value, column) in values.iter_mut().zip(&plan.columns) {
            let rows = sums
                .get((column.first - plan.first) as usize..(column.end - plan.first) as usize)
                .ok_or(Error::invalid_argument("image footprint outside rows"))?;
            let mut total = [0.0; N];
            for (row, sum) in (column.first..).zip(rows) {
                let weight = column.weight(row);
                for (total, sum) in total.iter_mut().zip(sum) {
                    *total += weight * sum;
                }
            }
            let normalization = column.normalization * span.normalization;
            *value = total.map(|v| v * normalization);
        }
        Ok(())
    }
}

/// How device pixels select source samples, as in the general sampler. The
/// flag tells a quarter turn, whose device axes exchange source axes.
#[derive(Clone, Copy)]
enum Footprint {
    /// Magnification without interpolation (8.9.5.3), per pixel when the
    /// placement is neither upright nor a quarter turn.
    Nearest(Option<bool>),
    /// Interpolated magnification of an upright or quarter-turned image.
    Bilinear(bool),
    /// Minification of an upright or quarter-turned image.
    Area(bool),
}
impl Footprint {
    fn new(sampler: &Sampler) -> Option<Self> {
        let t = sampler.inverse;
        let turned = if t.b == 0.0 && t.c == 0.0 {
            Some(false)
        } else if t.a == 0.0 && t.d == 0.0 {
            Some(true)
        } else {
            None
        };
        if sampler.minifying() {
            turned.map(Self::Area)
        } else if sampler.interpolate {
            turned.map(Self::Bilinear)
        } else {
            Some(Self::Nearest(turned))
        }
    }
}

enum Plan {
    Nearest(Option<Centres>),
    Bilinear(Centres),
    Area(Area),
}
impl Plan {
    fn new(footprint: Footprint, sampler: &Sampler, clip: &Mask) -> Result<Self> {
        Ok(match footprint {
            Footprint::Nearest(turned) => Self::Nearest(
                turned
                    .map(|swapped| Centres::new(sampler, clip, swapped))
                    .transpose()?,
            ),
            Footprint::Bilinear(swapped) => Self::Bilinear(Centres::new(sampler, clip, swapped)?),
            Footprint::Area(swapped) => Self::Area(Area::new(sampler, clip, swapped)?),
        })
    }

    fn bytes<const N: usize>(&self) -> usize {
        match self {
            Self::Nearest(centres) => centres.as_ref().map_or(0, Centres::bytes),
            Self::Bilinear(centres) => centres.bytes(),
            Self::Area(area) => area.bytes::<N>(),
        }
    }
}

/// Where one device pixel centre falls along one source axis, as sample
/// offsets: the nearest sample and the two bilinear neighbours.
#[derive(Clone, Copy)]
struct Tap {
    near: usize,
    low: usize,
    high: usize,
    fraction: f64,
}
impl Tap {
    fn new(center: f64, length: u32, stride: usize) -> Self {
        let maximum = length.saturating_sub(1);
        let offset = |value: f64| coordinate(value, maximum) as usize * stride;
        let position = center - 0.5;
        let low = position.floor();
        Self {
            near: offset(center.floor()),
            low: offset(low),
            high: offset(low + 1.0),
            fraction: position - low,
        }
    }
}

/// Taps of the clip's device columns and rows. In an upright or quarter-
/// turned placement each device axis moves along one source axis, so a
/// sample offset is the sum of a column tap and a row tap.
struct Centres {
    columns: Vec<Tap>,
    rows: Vec<Tap>,
}
impl Centres {
    fn new(sampler: &Sampler, clip: &Mask, swapped: bool) -> Result<Self> {
        let (width, height) = (sampler.width, sampler.height);
        let stride = width as usize;
        let (x, y) = clip.origin();
        // The other coordinate contributes an exact zero to each centre.
        let columns = (x..x + clip.width()).map(|x| {
            let center = sampler.center(x, y);
            if swapped {
                Tap::new(center.y, height, stride)
            } else {
                Tap::new(center.x, width, 1)
            }
        });
        let rows = (y..y + clip.height()).map(|y| {
            let center = sampler.center(x, y);
            if swapped {
                Tap::new(center.x, width, 1)
            } else {
                Tap::new(center.y, height, stride)
            }
        });
        Ok(Self {
            columns: collect(columns)?,
            rows: collect(rows)?,
        })
    }

    fn bytes(&self) -> usize {
        (self.columns.len() + self.rows.len()).saturating_mul(size_of::<Tap>())
    }

    /// The row tap and column taps of one coverage run.
    fn offsets(&self, row: usize, column: usize, length: usize) -> Result<(Tap, &[Tap])> {
        let row = *self
            .rows
            .get(row)
            .ok_or(Error::invalid_argument("image row outside centres"))?;
        let columns = self
            .columns
            .get(column..column + length)
            .ok_or(Error::invalid_argument("image run outside centres"))?;
        Ok((row, columns))
    }
}

/// The column and row footprints of one clip. A quarter turn exchanges the
/// source axes that device columns and rows step along.
struct Area {
    columns: Vec<Span>,
    rows: Vec<Span>,
    swapped: bool,
    /// Source cells under all column footprints: columns of an upright
    /// placement, lines of a quarter turn.
    first: u32,
    end: u32,
    /// Source cells under all column footprints.
    samples: usize,
}
impl Area {
    fn new(sampler: &Sampler, clip: &Mask, swapped: bool) -> Result<Self> {
        let t = sampler.inverse;
        let (width, height) = (sampler.width, sampler.height);
        let ((column_scale, column_offset, columns), (row_scale, row_offset, rows)) = if swapped {
            ((t.b, t.f, height), (t.c, t.e, width))
        } else {
            ((t.a, t.e, width), (t.d, t.f, height))
        };
        let (x, y) = clip.origin();
        let spans = |pixels: std::ops::Range<u32>, scale, offset, maximum| -> Result<Vec<Span>> {
            let mut spans = Vec::new();
            spans
                .try_reserve_exact(pixels.len())
                .map_err(|_| Error::limit("image footprint allocation failed"))?;
            for pixel in pixels {
                spans.push(Span::new(pixel, scale, offset, maximum)?);
            }
            Ok(spans)
        };
        let columns = spans(x..x + clip.width(), column_scale, column_offset, columns)?;
        let rows = spans(y..y + clip.height(), row_scale, row_offset, rows)?;
        let first = columns.iter().map(|c| c.first).min().unwrap_or(0);
        let end = columns.iter().map(|c| c.end).max().unwrap_or(0).max(first);
        let samples = columns
            .iter()
            .map(|c| (c.end - c.first) as usize)
            .fold(0, usize::saturating_add);
        Ok(Self {
            columns,
            rows,
            swapped,
            first,
            end,
            samples,
        })
    }

    /// Sums kept per device row: one per source cell under the columns, in
    /// the source line that an upright row accumulates or in the source
    /// lines under the columns of a quarter turn.
    fn lines(&self) -> usize {
        (self.end - self.first) as usize
    }

    fn bytes<const N: usize>(&self) -> usize {
        let codes = if self.swapped {
            0
        } else {
            self.lines().saturating_mul(size_of::<[u32; N]>())
        };
        (self.columns.len() + self.rows.len())
            .saturating_mul(size_of::<Span>())
            .saturating_add(self.columns.len().saturating_mul(size_of::<[f64; N]>()))
            .saturating_add(self.lines().saturating_mul(size_of::<[f64; N]>()))
            .saturating_add(codes)
    }
}

/// Opaque samples whose every code has an integer level: Decode ranges of
/// [0 1] or [1 0] scale codes by 255 / (2^bits - 1) (8.9.5.2), so sums of
/// codes are exact and an average level is an affine map of the mean code.
struct Levels<'a, const N: usize> {
    samples: &'a [[u8; N]],
    width: usize,
    offset: [f64; N],
    scale: [f64; N],
    table: [[u8; 256]; N],
}
impl<'a, const N: usize> Levels<'a, N> {
    /// Levels of `image`'s samples; with `colour_only`, an image with a soft
    /// mask too, whose mask the caller applies.
    fn new(image: &'a Image, colour_only: bool) -> Option<Self> {
        let bits = image.decoded.bits_per_component;
        let space = match N {
            1 => {
                matches!(image.space, Some(crate::color::Space::Gray))
                    && matches!(bits, 1 | 2 | 4 | 8)
            }
            3 => matches!(image.space, Some(crate::color::Space::Rgb)) && bits == 8,
            _ => false,
        };
        if !space
            || usize::from(image.decoded.components) != N
            || !(matches!(image.decoded.mask, MaskKind::None)
                || colour_only && matches!(image.decoded.mask, MaskKind::Soft(_)))
            || image.mask.is_some() && !colour_only
            || image.matte.is_some()
            || image.decode.len() != N
        {
            return None;
        }
        let Samples::Bytes(bytes) = &image.decoded.samples else {
            return None;
        };
        let (samples, rest) = bytes.as_chunks::<N>();
        let width = usize::try_from(image.decoded.width).ok()?;
        let height = usize::try_from(image.decoded.height).ok()?;
        if !rest.is_empty() || width.checked_mul(height) != Some(samples.len()) {
            return None;
        }
        let step = 255 / ((1u16 << bits) - 1);
        let mut offset = [0.0; N];
        let mut scale = [0.0; N];
        let mut table = [[0; 256]; N];
        for (channel, &range) in image.decode.iter().enumerate() {
            let inverted = if range == (0.0, 1.0) {
                false
            } else if range == (1.0, 0.0) {
                true
            } else {
                return None;
            };
            offset[channel] = if inverted { 255.0 } else { 0.0 };
            scale[channel] = if inverted { -1.0 } else { 1.0 } * f64::from(step);
            for (code, level) in (0u16..).zip(&mut table[channel]) {
                let value = u8::try_from(code.saturating_mul(step).min(255)).ok()?;
                *level = if inverted { 255 - value } else { value };
            }
        }
        Some(Self {
            samples,
            width,
            offset,
            scale,
            table,
        })
    }

    fn row(&self, row: u32) -> Result<&'a [[u8; N]]> {
        let start = (row as usize)
            .checked_mul(self.width)
            .ok_or(Error::limit("image row offset overflow"))?;
        self.samples
            .get(start..start + self.width)
            .ok_or(Error::syntax("missing image row"))
    }

    /// The RGB levels of a mean code.
    fn level(&self, code: [f64; N]) -> [f64; 3] {
        let level: [f64; N] = std::array::from_fn(|c| self.offset[c] + self.scale[c] * code[c]);
        std::array::from_fn(|c| level[c.min(N - 1)])
    }

    /// The RGB bytes of one sample.
    fn bytes(&self, sample: [u8; N]) -> [u8; 3] {
        let level: [u8; N] = std::array::from_fn(|c| self.table[c][usize::from(sample[c])]);
        std::array::from_fn(|c| level[c.min(N - 1)])
    }
}

/// The output pixels of one chunk, over the background they already hold.
struct Target<'a> {
    output: &'a mut Pixmap,
    tile: Tile,
    region: Tile,
    background: [u8; 4],
}
impl Target<'_> {
    fn row(&mut self, x: u32, y: u32, length: usize) -> Result<&mut [[u8; 4]]> {
        let start = usize::try_from(
            u64::from(y - self.tile.y) * u64::from(self.tile.width) + u64::from(x - self.tile.x),
        )
        .map_err(|_| Error::limit("image output index exceeds usize"))?;
        let row = self
            .output
            .data_mut()
            .get_mut(start * 4..(start + length) * 4)
            .ok_or(Error::invalid_argument("image outside output"))?;
        Ok(row.as_chunks_mut::<4>().0)
    }

    fn store(
        &mut self,
        x: u32,
        y: u32,
        coverage: &[u8],
        mut level: impl FnMut(usize) -> Result<Option<[f64; 3]>>,
    ) -> Result<()> {
        let full = coverage.first() == Some(&255);
        let mixed = (!full).then_some(self.background);
        let row = self.row(x, y, coverage.len())?;
        for (pixel, (index, &coverage)) in row.iter_mut().zip(coverage.iter().enumerate()) {
            // An empty footprint gives the group path zero opacity there.
            let Some(level) = level(index)? else {
                continue;
            };
            *pixel = match mixed {
                None => {
                    let [red, green, blue] = level.map(round);
                    [red, green, blue, 255]
                }
                Some(background) => mix(level, coverage, background),
            };
        }
        Ok(())
    }

    fn copy(
        &mut self,
        x: u32,
        y: u32,
        coverage: &[u8],
        mut bytes: impl FnMut(usize) -> Result<[u8; 3]>,
    ) -> Result<()> {
        let full = coverage.first() == Some(&255);
        let background = self.background;
        let row = self.row(x, y, coverage.len())?;
        for (pixel, (index, &coverage)) in row.iter_mut().zip(coverage.iter().enumerate()) {
            let [red, green, blue] = bytes(index)?;
            *pixel = if full {
                [red, green, blue, 255]
            } else {
                mix([red, green, blue].map(f64::from), coverage, background)
            };
        }
        Ok(())
    }

    fn nearest<const N: usize>(
        &mut self,
        clip: &Mask,
        sampler: &Sampler,
        centres: Option<&Centres>,
        levels: &Levels<'_, N>,
    ) -> Result<()> {
        let origin = clip.origin();
        for (row, column, coverage) in clip.spans() {
            let (x, y) = (origin.0 + index(column)?, origin.1 + index(row)?);
            let offsets = centres
                .map(|c| c.offsets(row, column, coverage.len()))
                .transpose()?;
            self.copy(x, y, coverage, |pixel| {
                let sample = match &offsets {
                    Some((row, columns)) => {
                        let column = columns
                            .get(pixel)
                            .ok_or(Error::invalid_argument("image run outside centres"))?;
                        row.near + column.near
                    }
                    None => sampler.nearest(x + index(pixel)?, y)?,
                };
                levels
                    .samples
                    .get(sample)
                    .map(|&sample| levels.bytes(sample))
                    .ok_or(Error::syntax("missing image sample"))
            })?;
        }
        Ok(())
    }
}

/// Mixes a partially covered level with the opaque background, rounding once.
fn mix(level: [f64; 3], coverage: u8, background: [u8; 4]) -> [u8; 4] {
    let opacity = f64::from(coverage) / 255.0;
    let [red, green, blue] =
        [0, 1, 2].map(|c| round(level[c] * opacity + f64::from(background[c]) * (1.0 - opacity)));
    [red, green, blue, 255]
}

/// Footprint sums of one source line in code units: the weighted end cells
/// around an exact integer interior sum.
fn line_sums<const N: usize>(
    line: &[[u8; N]],
    spans: &[Span],
    sums: &mut [[f64; N]],
) -> Result<()> {
    for (span, sum) in spans.iter().zip(sums) {
        let cells = line
            .get(span.first as usize..span.end as usize)
            .ok_or(Error::syntax("image footprint outside samples"))?;
        *sum = match cells {
            [] => [0.0; N],
            [only] => only.map(|v| span.first_weight * f64::from(v)),
            [first, interior @ .., last] => {
                let mut total = [0.0; N];
                for run in interior.chunks(RUN) {
                    // A run's codes sum below 2^32, so wrapping additions are
                    // exact and, free of overflow checks, vectorize.
                    let mut codes = [0u32; N];
                    for sample in run {
                        for (code, &value) in codes.iter_mut().zip(sample) {
                            *code = code.wrapping_add(u32::from(value));
                        }
                    }
                    for (total, code) in total.iter_mut().zip(codes) {
                        *total += f64::from(code);
                    }
                }
                std::array::from_fn(|c| {
                    span.first_weight * f64::from(first[c])
                        + total[c]
                        + span.last_weight * f64::from(last[c])
                })
            }
        };
    }
    Ok(())
}

/// Collects a bounded number of plan entries with a fallible reservation.
fn collect<T>(values: impl ExactSizeIterator<Item = T>) -> Result<Vec<T>> {
    let mut out = Vec::new();
    out.try_reserve_exact(values.len())
        .map_err(|_| Error::limit("image footprint allocation failed"))?;
    out.extend(values);
    Ok(out)
}

fn filled<T: Clone>(count: usize, value: T) -> Result<Vec<T>> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| Error::limit("image footprint allocation failed"))?;
    values.resize(count, value);
    Ok(values)
}

fn index(value: usize) -> Result<u32> {
    u32::try_from(value).map_err(|_| Error::limit("image coordinate exceeds u32"))
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a finite level is clamped to the byte range before rounding"
)]
fn round(level: f64) -> u8 {
    (level.clamp(0.0, 255.0) + 0.5).floor().min(255.0) as u8
}

#[cfg(test)]
mod tests {
    use crate::color::{Color, Space};
    use crate::display::{Command, DisplayList, ImageCommand};
    use crate::interpreter::images::Image;
    use crate::transparency::{Attributes, Composite};
    use crate::{RenderOptions, Tile};
    use pagura_core::{Limits, Position, Rect, Value};
    use pagura_image::{DecodedImage, ImageKind, MaskKind, Samples};
    use pagura_raster::Transform;
    use std::sync::Arc;

    /// An exact rational for the independent oracle.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct Q(i128, i128);
    impl Q {
        fn new(n: i128, d: i128) -> Self {
            let g = gcd(n.abs(), d.abs()).max(1);
            let s = if d < 0 { -1 } else { 1 };
            Self(s * n / g, s * d / g)
        }
        fn int(n: i128) -> Self {
            Self(n, 1)
        }
        /// Every test geometry is dyadic, hence exactly representable.
        fn of(value: f64) -> Self {
            let scaled = value * 1024.0;
            assert_eq!(scaled.fract(), 0.0, "non-dyadic test geometry {value}");
            #[expect(clippy::cast_possible_truncation, reason = "small dyadic test values")]
            Self::new(scaled as i128, 1024)
        }
        fn add(self, o: Self) -> Self {
            Self::new(self.0 * o.1 + o.0 * self.1, self.1 * o.1)
        }
        fn sub(self, o: Self) -> Self {
            self.add(Self(-o.0, o.1))
        }
        fn mul(self, o: Self) -> Self {
            Self::new(self.0 * o.0, self.1 * o.1)
        }
        fn div(self, o: Self) -> Self {
            Self::new(self.0 * o.1, self.1 * o.0)
        }
        fn min(self, o: Self) -> Self {
            if self.0 * o.1 <= o.0 * self.1 {
                self
            } else {
                o
            }
        }
        fn max(self, o: Self) -> Self {
            if self.0 * o.1 >= o.0 * self.1 {
                self
            } else {
                o
            }
        }
        fn floor(self) -> i128 {
            self.0.div_euclid(self.1)
        }
        /// Half-up rounding, and whether the value is an exact half.
        fn round(self) -> (i128, bool) {
            let twice = Self::new(2 * self.0 + self.1, 2 * self.1);
            (twice.floor(), (2 * self.0).rem_euclid(2 * self.1) == self.1)
        }
    }
    fn gcd(a: i128, b: i128) -> i128 {
        if b == 0 { a } else { gcd(b, a % b) }
    }

    const PAGE: u32 = 8;

    struct Case {
        width: u32,
        height: u32,
        components: u8,
        bits: u8,
        codes: Vec<u8>,
        inverted: bool,
        interpolate: bool,
        /// Unit square to device pixels.
        placement: [f64; 6],
    }

    impl Case {
        fn list(&self) -> DisplayList {
            let n = usize::from(self.components);
            let decoded = DecodedImage {
                width: self.width,
                height: self.height,
                bits_per_component: self.bits,
                components: self.components,
                samples: Samples::Bytes(self.codes.clone()),
                kind: ImageKind::Color {
                    color_space: Some(Value::name(if n == 1 {
                        b"DeviceGray"
                    } else {
                        b"DeviceRGB"
                    })),
                },
                decode: None,
                mask: MaskKind::None,
                interpolate: self.interpolate,
                reduction: None,
            };
            let image = Image {
                decoded,
                space: Some(if n == 1 { Space::Gray } else { Space::Rgb }),
                mask: None,
                matte: None,
                decode: vec![
                    if self.inverted {
                        (1.0, 0.0)
                    } else {
                        (0.0, 1.0)
                    };
                    n
                ],
                intent: None,
            };
            // The page maps user (x, y) to device (x, 8 - y), an involution.
            let [a, b, c, d, e, f] = self.placement;
            let page = Transform::new(1.0, 0.0, 0.0, -1.0, 0.0, f64::from(PAGE));
            DisplayList {
                commands: vec![Command::Image(ImageCommand {
                    image: Arc::new(image),
                    transform: Transform::new(a, b, c, d, e, f).then(page),
                    clip: None,
                    stencil: Color::default(),
                    composite: Composite::opaque(),
                    transfer: None,
                    position: Position::Unknown,
                })],
                crop: Rect::from_corners(0.0, 0.0, f64::from(PAGE), f64::from(PAGE)).unwrap(),
                rotation: pagura_document::Rotation::None,
                user_unit: 1.0,
                limits: Limits::default(),
                group: Attributes::default(),
                print_intent: false,
                separations: std::sync::OnceLock::new(),
            }
        }

        fn level(&self, column: i128, row: i128, channel: usize) -> Q {
            let n = usize::from(self.components);
            let index =
                usize::try_from(row * i128::from(self.width) + column).unwrap() * n + channel;
            let step = 255 / ((1 << self.bits) - 1);
            let level = i128::from(self.codes[index]) * step;
            Q::int(if self.inverted { 255 - level } else { level })
        }

        /// The device rectangle of the unit-square interval product, exactly.
        fn rect(&self, u: (Q, Q), v: (Q, Q)) -> (Q, Q, Q, Q) {
            let [a, b, c, d, e, f] = self.placement.map(Q::of);
            let map = |u: Q, v: Q| (u.mul(a).add(v.mul(c)).add(e), u.mul(b).add(v.mul(d)).add(f));
            let (p, q) = (map(u.0, v.0), map(u.1, v.1));
            (p.0.min(q.0), p.0.max(q.0), p.1.min(q.1), p.1.max(q.1))
        }

        /// Exact expected RGB levels at device pixel (x, y) over white, and
        /// whether a channel is an exact rounding tie.
        fn oracle(&self, x: i128, y: i128) -> [(i128, bool); 3] {
            let one = Q::int(1);
            let (w, h) = (i128::from(self.width), i128::from(self.height));
            let overlap = |lo: Q, hi: Q, p: i128| {
                let (a, b) = (lo.max(Q::int(p)), hi.min(Q::int(p + 1)));
                if a.0 * b.1 < b.0 * a.1 {
                    b.sub(a)
                } else {
                    Q::int(0)
                }
            };
            let full = self.rect((Q::int(0), one), (Q::int(0), one));
            let covered = overlap(full.0, full.1, x).mul(overlap(full.2, full.3, y));
            // The rasterizer's coverage byte for an axis-aligned rectangle.
            let coverage = covered.mul(Q::int(255)).round().0;
            let [a, b, c, d, ..] = self.placement;
            let scale = |p: f64, q: f64| p.hypot(q);
            // Main's sampler minifies when either source axis shrinks.
            let minifying = f64::from(self.width) / scale(a, b) > 1.0 + 1e-9
                || f64::from(self.height) / scale(c, d) > 1.0 + 1e-9;
            let channels = usize::from(self.components);
            std::array::from_fn(|channel| {
                let channel = channel.min(channels - 1);
                let image = if minifying {
                    let (mut sum, mut area) = (Q::int(0), Q::int(0));
                    for row in 0..h {
                        for column in 0..w {
                            let cell = self.rect(
                                (Q::new(column, w), Q::new(column + 1, w)),
                                (Q::new(h - row - 1, h), Q::new(h - row, h)),
                            );
                            let part = overlap(cell.0, cell.1, x).mul(overlap(cell.2, cell.3, y));
                            sum = sum.add(part.mul(self.level(column, row, channel)));
                            area = area.add(part);
                        }
                    }
                    if area.0 == 0 {
                        Q::int(255)
                    } else {
                        sum.div(area)
                    }
                } else {
                    self.magnified(x, y, channel)
                };
                if coverage == 0 {
                    return (255, false);
                }
                let mixed = image
                    .mul(Q::int(coverage))
                    .add(Q::int(255 * (255 - coverage)))
                    .div(Q::int(255));
                mixed.round()
            })
        }

        /// Nearest or bilinear sampling at the pixel centre (8.9.5.3).
        fn magnified(&self, x: i128, y: i128, channel: usize) -> Q {
            let [a, b, c, d, e, f] = self.placement.map(Q::of);
            let (cx, cy) = (Q::new(2 * x + 1, 2), Q::new(2 * y + 1, 2));
            // Invert the unit-square map, then scale to source cells.
            let det = a.mul(d).sub(b.mul(c));
            let (px, py) = (cx.sub(e), cy.sub(f));
            let u = px.mul(d).sub(py.mul(c)).div(det);
            let v = py.mul(a).sub(px.mul(b)).div(det);
            let (w, h) = (i128::from(self.width), i128::from(self.height));
            let column = u.mul(Q::int(w));
            let row = Q::int(h).sub(v.mul(Q::int(h)));
            let clamp = |value: i128, length: i128| value.clamp(0, length - 1);
            if !self.interpolate {
                return self.level(clamp(column.floor(), w), clamp(row.floor(), h), channel);
            }
            let half = Q::new(1, 2);
            let (column, row) = (column.sub(half), row.sub(half));
            let (left, top) = (column.floor(), row.floor());
            let (fx, fy) = (column.sub(Q::int(left)), row.sub(Q::int(top)));
            let one = Q::int(1);
            let mut sum = Q::int(0);
            for (dx, wx) in [(0, one.sub(fx)), (1, fx)] {
                for (dy, wy) in [(0, one.sub(fy)), (1, fy)] {
                    let level = self.level(clamp(left + dx, w), clamp(top + dy, h), channel);
                    sum = sum.add(level.mul(wx).mul(wy));
                }
            }
            sum
        }
    }

    fn cases() -> Vec<Case> {
        let placements = [
            // Upright and reflected minification with fractional edges.
            [4.0, 0.0, 0.0, -3.0, 0.5, 6.25],
            [-4.0, 0.0, 0.0, 3.0, 7.5, 1.75],
            // Exact two-to-one cells, where half-level ties are common.
            [3.0, 0.0, 0.0, -2.0, 1.0, 6.0],
            // Quarter turns.
            [0.0, 3.0, 4.0, 0.0, 1.25, 0.5],
            [0.0, -3.0, -4.0, 0.0, 6.75, 7.5],
            // Magnification, upright and turned.
            [7.5, 0.0, 0.0, -6.5, 0.25, 7.0],
            [0.0, 7.0, 7.5, 0.0, 0.5, 0.75],
        ];
        let mut cases = Vec::new();
        let images: [(u8, u8, bool); 6] = [
            (1, 8, false),
            (1, 1, false),
            (1, 1, true),
            (1, 2, true),
            (1, 4, false),
            (3, 8, false),
        ];
        for placement in placements {
            for (components, bits, inverted) in images {
                for interpolate in [false, true] {
                    let count = 6 * 4 * usize::from(components);
                    let codes = (0..count)
                        .map(|i| u8::try_from((i * 37 + i * i * 11) % (1 << bits)).unwrap())
                        .collect();
                    cases.push(Case {
                        width: 6,
                        height: 4,
                        components,
                        bits,
                        codes,
                        inverted,
                        interpolate,
                        placement,
                    });
                }
            }
        }
        cases
    }

    #[test]
    fn direct_images_round_exact_rational_oracles_once() {
        let options = RenderOptions::default();
        let (mut pixels, mut ties) = (0, 0);
        for case in cases() {
            let list = case.list();
            let output = list.render(&options).unwrap();
            for y in 0..PAGE {
                for x in 0..PAGE {
                    let index = usize::try_from(y * PAGE + x).unwrap() * 4;
                    let actual = &output.data()[index..index + 4];
                    for (channel, (expected, tie)) in case
                        .oracle(i128::from(x), i128::from(y))
                        .into_iter()
                        .enumerate()
                    {
                        let actual = i128::from(actual[channel]);
                        pixels += 1;
                        if tie {
                            // Exact ties are outside the contract (ADR 0004).
                            ties += 1;
                            assert!((expected - 1..=expected).contains(&actual));
                        } else {
                            assert_eq!(
                                actual, expected,
                                "{:?} bits {} at ({x},{y}) channel {channel}",
                                case.placement, case.bits
                            );
                        }
                    }
                    assert_eq!(actual[3], 255);
                }
            }
        }
        assert!(ties > 0 && ties < pixels / 10, "{ties} ties of {pixels}");
    }

    #[test]
    fn direct_images_paint_and_match_the_group_compositor() {
        let options = RenderOptions::default();
        for case in cases() {
            let list = case.list();
            let [Command::Image(command)] = list.commands.as_slice() else {
                unreachable!();
            };
            let separations = list.preview_separations(&options).unwrap();
            let plan = super::super::plan::page(&list, &separations).unwrap();
            let mut context =
                super::super::Context::new(&list, &options, &plan, separations).unwrap();
            let tile = Tile {
                x: 0,
                y: 0,
                width: PAGE,
                height: PAGE,
            };
            let mut direct =
                pagura_raster::Pixmap::new_with_limits(PAGE, PAGE, options.background, list.limits)
                    .unwrap();
            assert!(
                context
                    .paint_opaque_image(
                        command,
                        super::Model::Rgb,
                        &mut direct,
                        (tile, tile),
                        [255; 4]
                    )
                    .unwrap()
            );
            // A knockout page group composites its one mark identically but
            // never takes the direct path.
            let mut group = case.list();
            group.group.knockout = true;
            let general = group.render(&options).unwrap();
            for (index, (a, b)) in direct.data().iter().zip(general.data()).enumerate() {
                assert!(
                    a.abs_diff(*b) <= 1,
                    "{:?} byte {index}: {a} {b}",
                    case.placement
                );
            }
            assert_eq!(list.render(&options).unwrap(), direct);
        }
    }
}
