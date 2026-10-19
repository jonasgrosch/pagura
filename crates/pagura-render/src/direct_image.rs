//! Fused decoding, colour conversion and area filtering for a page whose only
//! mark is one opaque image ([`ImageReconstruction::Fused`]).
//!
//! The image is decoded in the deferred layout (`pagura_image::deferred`):
//! a baseline YCbCr JPEG stays in its component planes, with one-level luma
//! intervals, and one-bit samples stay packed. Each device row then takes
//! the exact-area footprints of the general compositor on the original
//! sample grid. Source rows are converted only when a footprint needs them,
//! equal rows are filtered once, and background JPEG blocks are skipped.
//! Interior cells of a footprint add up as integers; the end cells, the
//! row accumulation and partial pixel coverage use f32 and f64 weights, and
//! each pixel is rounded once. Every pixel is within one level of the
//! [`ImageReconstruction::Exact`] result.
//!
//! Pages, images and placements outside this scope return `None` and render
//! exactly. So do pages for which this path reports an error, so that errors
//! are those of the exact path.
//!
//! [`ImageReconstruction::Fused`]: crate::ImageReconstruction::Fused
//! [`ImageReconstruction::Exact`]: crate::ImageReconstruction::Exact
mod aligned;
mod area;
mod binary;
mod bytes;
mod footprint;
mod lanes;
mod sparse;

use crate::{
    DisplayList, ImageReconstruction, Pixmap, RenderOptions, Tile, color::Space, display::Command,
    interpreter::images::Image,
};
use pagura_core::{Error, Limits, Result};
use pagura_image::{MaskKind, Samples, deferred::SampleLayout};
use pagura_raster::Transform;

/// The widest footprint, in source samples along either axis, whose f32
/// accumulation stays well within half a level.
const MAXIMUM_SCALE: f64 = 63.0;

/// Compiles and renders a page, or one tile of it, painting directly when
/// `options.images` is [`ImageReconstruction::Fused`] and the page qualifies.
/// Tiles are cut from the directly painted page, so they assemble into it.
pub(crate) fn render(
    compile: impl Fn(SampleLayout) -> Result<DisplayList>,
    options: &RenderOptions,
    tile: Option<Tile>,
) -> Result<Pixmap> {
    if options.images == ImageReconstruction::Fused
        && let Ok(list) = compile(SampleLayout::Deferred)
    {
        if let Some(page) = paint(&list, options) {
            let Some(tile) = tile else {
                return Ok(page);
            };
            if let Some(pixmap) = cut(&page, tile, list.limits) {
                return Ok(pixmap);
            }
        }
        // A list without deferred samples is the exact list.
        if !deferred(&list.commands) {
            return finish(&list, options, tile);
        }
    }
    finish(&compile(SampleLayout::Interleaved)?, options, tile)
}

fn finish(list: &DisplayList, options: &RenderOptions, tile: Option<Tile>) -> Result<Pixmap> {
    match tile {
        None => list.render(options),
        Some(tile) => list.render_tile(options, tile),
    }
}

/// Whether any image in `commands` holds deferred samples.
fn deferred(commands: &[Command]) -> bool {
    commands.iter().any(|command| match command {
        Command::Image(image) => matches!(
            image.image.decoded.samples,
            Samples::Planar(_) | Samples::Packed(_)
        ),
        Command::Group(group) => deferred(&group.commands),
        Command::Path(_) | Command::Shading(_) | Command::Glyphs(_) => false,
    })
}

/// A copy of `tile` within the page; None for a tile outside it.
fn cut(page: &Pixmap, tile: Tile, limits: Limits) -> Option<Pixmap> {
    if tile.width == 0
        || tile.height == 0
        || tile.x.checked_add(tile.width)? > page.width()
        || tile.y.checked_add(tile.height)? > page.height()
    {
        return None;
    }
    let mut pixmap =
        Pixmap::new_with_limits(tile.width, tile.height, pagura_raster::Rgba8::WHITE, limits)
            .ok()?;
    let stride = page.width() as usize * 4;
    let (x, width) = (tile.x as usize * 4, tile.width as usize * 4);
    for (row, out) in page
        .data()
        .chunks_exact(stride)
        .skip(tile.y as usize)
        .zip(pixmap.data_mut().chunks_exact_mut(width))
    {
        out.copy_from_slice(&row[x..x + width]);
    }
    Some(pixmap)
}

/// Paints `list` if it is one opaque Gray or RGB image this path supports.
fn paint(list: &DisplayList, options: &RenderOptions) -> Option<Pixmap> {
    paint_checked(list, options).ok().flatten()
}

fn paint_checked(list: &DisplayList, options: &RenderOptions) -> Result<Option<Pixmap>> {
    let [Command::Image(command)] = list.commands.as_slice() else {
        return Ok(None);
    };
    let image = &command.image;
    let components = usize::from(image.decoded.components);
    if !command.composite.simple()
        || command.composite.overprint
        || command.transfer.is_some()
        || command.clip.is_some()
        || image.mask.is_some()
        || image.matte.is_some()
        || !matches!(image.decoded.mask, MaskKind::None)
        || !matches!(
            (&image.space, components),
            (Some(Space::Gray), 1) | (Some(Space::Rgb), 3)
        )
        || list.group.view.is_some()
        || list.group.knockout
        || list
            .group
            .model
            .is_some_and(|m| m != crate::transparency::Model::Rgb)
        || !matches!(image.decoded.bits_per_component, 1 | 2 | 4 | 8)
        || options.background.a != 255
        || list.preview_separations(options)?.enabled
    {
        return Ok(None);
    }
    let Some(placement) = Placement::new(list, options, command.transform)? else {
        return Ok(None);
    };
    let t = source_transform(image, &placement)?;
    // Wider footprints keep the general compositor and its f64 evaluation.
    if t.a.hypot(t.b).max(t.c.hypot(t.d)) > MAXIMUM_SCALE {
        return Ok(None);
    }
    if let Samples::Planar(planar) = &image.decoded.samples
        && (planar.width() > usize::from(u16::MAX)
            || planar.chroma_repeat().is_none()
            || image.decode.iter().any(|&range| range != (0.0, 1.0)))
    {
        return Ok(None);
    }
    if let Some(pixmap) = aligned::paint(image, &placement, options, list.limits)? {
        return Ok(Some(pixmap));
    }
    match &image.decoded.samples {
        Samples::Planar(_) | Samples::Packed(_) => {
            area::paint(image, &[], &placement, options, list.limits).map(Some)
        }
        Samples::Bytes(samples) => {
            area::paint(image, samples, &placement, options, list.limits).map(Some)
        }
        Samples::Words(_) | Samples::Mixed { .. } => Ok(None),
    }
}

/// An upright or quarter-turned placement on the device surface.
struct Placement {
    width: u32,
    height: u32,
    inverse: Transform,
    /// Device pixels touched: left, right, top and bottom.
    range: [usize; 4],
    /// The covered device rectangle: x0, x1, y0 and y1.
    extent: [f64; 4],
    rotated: bool,
}

impl Placement {
    fn new(
        list: &DisplayList,
        options: &RenderOptions,
        transform: Transform,
    ) -> Result<Option<Self>> {
        let device = list.device_transform(options);
        let transform = transform.then(device);
        if !((transform.b == 0.0 && transform.c == 0.0)
            || (transform.a == 0.0 && transform.d == 0.0))
            || !transform.is_finite()
        {
            return Ok(None);
        }
        let Some(inverse) = transform.inverse() else {
            return Ok(None);
        };
        let (width, height) = list.device_size(options)?;
        let (x0, x1) = ordered(transform.e, transform.e + transform.a + transform.c);
        let (y0, y1) = ordered(transform.f, transform.f + transform.d + transform.b);
        let crop0 = device.apply(pagura_raster::Point::new(list.crop.x0, list.crop.y0));
        let crop1 = device.apply(pagura_raster::Point::new(list.crop.x1, list.crop.y1));
        let (cx0, cx1) = ordered(crop0.x, crop1.x);
        let (cy0, cy1) = ordered(crop0.y, crop1.y);
        let (x0, x1, y0, y1) = (
            x0.max(cx0).max(0.0),
            x1.min(cx1).min(f64::from(width)),
            y0.max(cy0).max(0.0),
            y1.min(cy1).min(f64::from(height)),
        );
        if x0 >= x1 || y0 >= y1 {
            return Ok(None);
        }
        let (left, right, top, bottom) = (
            index(x0.floor()),
            index(x1.ceil()),
            index(y0.floor()),
            index(y1.ceil()),
        );
        let work = (right - left).saturating_mul(bottom - top);
        if work > list.limits.raster_work {
            return Err(Error::limit("image raster work exceeded"));
        }
        Ok(Some(Self {
            width,
            height,
            inverse,
            range: [left, right, top, bottom],
            extent: [x0, x1, y0, y1],
            rotated: transform.a == 0.0,
        }))
    }
}

/// Nearest-sample placement, for magnified images and aligned grids.
fn nearest(
    image: &Image,
    samples: &[u8],
    placement: &Placement,
    options: &RenderOptions,
    limits: Limits,
) -> Result<Pixmap> {
    let Placement {
        width,
        height: _,
        inverse: _,
        range: [left, right, top, bottom],
        extent,
        rotated,
    } = *placement;
    let components = usize::from(image.decoded.components);
    let mut target = Pixmap::new_with_limits(width, placement.height, options.background, limits)?;
    let temporary = (right - left)
        .checked_mul(size_of::<usize>())
        .and_then(|n| n.checked_add(target.data().len()))
        .and_then(|n| n.checked_add(samples.len()));
    if temporary.is_none_or(|n| n > limits.decoded_bytes) {
        return Err(Error::limit("image sampling storage exceeded"));
    }
    let mut cached_row = Vec::new();
    let mut previous = None;
    let source_rows = if rotated {
        image.decoded.width
    } else {
        image.decoded.height
    };
    if (source_rows as usize) < bottom - top {
        let bytes = (right - left) * 4;
        if temporary
            .and_then(|n| n.checked_add(bytes))
            .is_none_or(|n| n > limits.decoded_bytes)
        {
            return Err(Error::limit("image row cache storage exceeded"));
        }
        cached_row.resize(bytes, 0);
    }
    let source_width = image.decoded.width as usize;
    let source_height = image.decoded.height as usize;
    let inverse = source_transform(image, placement)?;
    let tables = decode_tables(image);
    let identity = image.decoded.bits_per_component == 8
        && image.decode.iter().all(|&range| range == (0.0, 1.0));
    let columns: Vec<usize> = (left..right)
        .map(|x| {
            if rotated {
                index((x as f64 + 0.5) * inverse.b + inverse.f).min(source_height - 1)
                    * source_width
                    * components
            } else {
                index((x as f64 + 0.5) * inverse.a + inverse.e).min(source_width - 1) * components
            }
        })
        .collect();
    let contiguous = components == 3 && identity && columns.windows(2).all(|w| w[1] == w[0] + 3);
    let format = Format {
        tables: &tables,
        components,
        identity,
        contiguous,
    };
    for (y, row) in target
        .data_mut()
        .chunks_exact_mut(width as usize * 4)
        .enumerate()
        .take(bottom)
        .skip(top)
    {
        let offset = if rotated {
            index((y as f64 + 0.5) * inverse.c + inverse.e).min(source_width - 1) * components
        } else {
            index((y as f64 + 0.5) * inverse.d + inverse.f).min(source_height - 1)
                * source_width
                * components
        };
        let source = &samples[offset..];
        let row = &mut row[left * 4..right * 4];
        if cached_row.is_empty() {
            format.blit(row, source, &columns);
        } else {
            if previous != Some(offset) {
                format.blit(&mut cached_row, source, &columns);
                previous = Some(offset);
            }
            row.copy_from_slice(&cached_row);
        }
        cover_row(row, left, y, extent, options.background);
    }
    Ok(target)
}

/// Maps device coordinates to the image's source sample grid.
fn source_transform(image: &Image, placement: &Placement) -> Result<Transform> {
    let width = f64::from(image.decoded.width);
    let height = f64::from(image.decoded.height);
    let transform = placement
        .inverse
        .then(Transform::new(width, 0.0, 0.0, -height, 0.0, height));
    if !transform.is_finite() {
        return Err(Error::limit("image filter coordinates overflow"));
    }
    Ok(transform)
}

fn decode_tables(image: &Image) -> Vec<[u8; 256]> {
    image
        .decode
        .iter()
        .map(|&(lo, hi)| {
            std::array::from_fn(|i| {
                byte(
                    (lo + i as f64 / f64::from((1u16 << image.decoded.bits_per_component) - 1)
                        * (hi - lo))
                        * 255.0,
                )
            })
        })
        .collect()
}

/// Mixes the partly covered pixels of a device row with the background.
fn cover_row(
    row: &mut [u8],
    left: usize,
    y: usize,
    [x0, x1, y0, y1]: [f64; 4],
    background: pagura_raster::Rgba8,
) {
    let right = left + row.len() / 4;
    let coverage_y = overlap(y, y0, y1);
    if coverage_y < 1.0 {
        for (x, pixel) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            cover(pixel, coverage_y * overlap(x + left, x0, x1), background);
        }
    } else {
        if let Some(pixel) = row.first_chunk_mut::<4>() {
            cover(pixel, overlap(left, x0, x1), background);
        }
        if row.len() > 4
            && let Some(pixel) = row.last_chunk_mut::<4>()
        {
            cover(pixel, overlap(right - 1, x0, x1), background);
        }
    }
}

/// How nearest samples become RGBA bytes.
struct Format<'a> {
    tables: &'a [[u8; 256]],
    components: usize,
    identity: bool,
    contiguous: bool,
}

impl Format<'_> {
    fn blit(&self, row: &mut [u8], source: &[u8], columns: &[usize]) {
        let pixels = row.as_chunks_mut::<4>().0;
        if self.contiguous {
            let start = columns.first().copied().unwrap_or(0);
            let source = &source[start..start + columns.len() * 3];
            for (out, rgb) in pixels.iter_mut().zip(source.as_chunks::<3>().0) {
                *out = [rgb[0], rgb[1], rgb[2], 255];
            }
        } else if self.components == 3 && self.identity {
            for (out, &sx) in pixels.iter_mut().zip(columns) {
                *out = [source[sx], source[sx + 1], source[sx + 2], 255];
            }
        } else if self.components == 3 {
            for (out, &sx) in pixels.iter_mut().zip(columns) {
                *out = [
                    self.tables[0][usize::from(source[sx])],
                    self.tables[1][usize::from(source[sx + 1])],
                    self.tables[2][usize::from(source[sx + 2])],
                    255,
                ];
            }
        } else {
            for (out, &sx) in pixels.iter_mut().zip(columns) {
                let value = self.tables[0][usize::from(source[sx])];
                *out = [value, value, value, 255];
            }
        }
    }
}

fn ordered(a: f64, b: f64) -> (f64, f64) {
    (a.min(b), a.max(b))
}

/// The covered fraction of pixel `pixel` along one axis.
fn overlap(pixel: usize, low: f64, high: f64) -> f64 {
    ((pixel as f64 + 1.0).min(high) - (pixel as f64).max(low)).clamp(0.0, 1.0)
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "coordinates clamped to an allocated image or surface"
)]
fn index(value: f64) -> usize {
    value.max(0.0) as usize
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped byte"
)]
fn byte(value: f64) -> u8 {
    value.round().clamp(0.0, 255.0) as u8
}

fn cover(pixel: &mut [u8; 4], coverage: f64, background: pagura_raster::Rgba8) {
    if coverage == 1.0 {
        return;
    }
    for (out, under) in pixel[..3]
        .iter_mut()
        .zip([background.r, background.g, background.b])
    {
        *out = byte(f64::from(*out) * coverage + f64::from(under) * (1.0 - coverage));
    }
}

/// A zero-filled vector, reporting allocation failure as a limit error.
fn filled<T: Clone>(length: usize, value: T) -> Result<Vec<T>> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(length)
        .map_err(|_| Error::limit("image filter allocation failed"))?;
    values.resize(length, value);
    Ok(values)
}

#[cfg(test)]
mod tests;
