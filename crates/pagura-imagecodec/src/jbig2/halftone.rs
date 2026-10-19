//! Pattern dictionaries (6.7/7.4.4) and halftones (6.6/7.4.5), T.88 (02/2000).
use super::{
    Bitmap,
    arithmetic::{Context, Decoder},
    budget::Work,
    generic::{self, Template},
    mmr,
    reader::Reader,
};
use pagura_core::{Error, Limits, Position, Result};

pub(super) struct Dictionary {
    pub(super) patterns: Vec<Bitmap>,
    pub(super) bytes: usize,
}
pub(super) fn dictionary(
    reader: &mut Reader<'_>,
    limits: &Limits,
    work: &mut Work,
) -> Result<Dictionary> {
    let flags = reader.byte()?;
    if flags & !7 != 0 || flags & 1 != 0 && flags & 6 != 0 {
        return Err(Error::syntax("invalid JBIG2 pattern dictionary flags"));
    }
    let width = u32::from(reader.byte()?);
    let height = u32::from(reader.byte()?);
    let count = u64::from(reader.u32()?) + 1;
    if width == 0 || height == 0 {
        return Err(Error::syntax("empty JBIG2 pattern"));
    }
    if count > limits.value_nodes as u64 {
        return Err(Error::limit("JBIG2 pattern count exceeds budget"));
    }
    let collective_width = u32::try_from(count * u64::from(width))
        .map_err(|_| Error::limit("JBIG2 pattern width overflow"))?;
    let count = usize::try_from(count).map_err(|_| Error::limit("JBIG2 pattern count overflow"))?;
    work.operations(count, limits)?;
    work.pixels(u64::from(collective_width) * u64::from(height) * 2, limits)?;
    let template = Template::prescribed(
        (flags >> 1) & 3,
        Some(-i32::try_from(width).map_err(|_| Error::limit("JBIG2 pattern width overflow"))?),
    );
    let (_, collective_bytes) = super::bitmap::layout(collective_width, height, limits)?;
    let (_, pattern_bytes) = super::bitmap::layout(width, height, limits)?;
    let bytes = count
        .checked_mul(pattern_bytes + size_of::<Bitmap>())
        .ok_or_else(|| Error::limit("JBIG2 patterns size overflow"))?;
    let peak = bytes
        .checked_add(collective_bytes)
        .and_then(|n| {
            n.checked_add(if flags & 1 == 0 {
                template.context_bytes()
            } else {
                0
            })
        })
        .ok_or_else(|| Error::limit("JBIG2 patterns size overflow"))?;
    if peak > limits.decoded_bytes {
        return Err(Error::limit("JBIG2 patterns exceed byte budget"));
    }
    let base = match reader.position() {
        Position::Buffer(n) => n,
        _ => 0,
    };
    let data = reader.rest();
    let collective = if flags & 1 != 0 {
        mmr::decode(data, collective_width, height, limits, base)?
    } else {
        let mut contexts = vec![Context::default(); template.context_count()];
        generic::decode_arithmetic(
            collective_width,
            height,
            &template,
            false,
            &mut Decoder::new(data, base)?,
            &mut contexts,
            limits,
        )?
    };
    let mut patterns = Vec::with_capacity(count);
    for n in 0..u32::try_from(count).map_err(|_| Error::limit("JBIG2 pattern count overflow"))? {
        let mut pattern = Bitmap::new(width, height, false, limits)?;
        for y in 0..height {
            for x in 0..width {
                pattern.set(
                    x,
                    y,
                    collective.pixel(i64::from(n * width + x), i64::from(y)),
                );
            }
        }
        patterns.push(pattern);
    }
    Ok(Dictionary { patterns, bytes })
}

struct Grid {
    width: u32,
    height: u32,
    x: i64,
    y: i64,
    rx: i64,
    ry: i64,
}
impl Grid {
    fn position(&self, n: u32, m: u32) -> (i64, i64) {
        (
            (self.x + i64::from(m) * self.ry + i64::from(n) * self.rx) >> 8,
            (self.y + i64::from(m) * self.rx - i64::from(n) * self.ry) >> 8,
        )
    }
}

pub(super) fn region(
    reader: &mut Reader<'_>,
    size: (u32, u32),
    dictionary: &Dictionary,
    limits: &Limits,
    work: &mut Work,
) -> Result<Bitmap> {
    let flags = reader.byte()?;
    let mmr = flags & 1 != 0;
    let kind = (flags >> 1) & 3;
    let skip_enabled = flags & 8 != 0;
    let operator = (flags >> 4) & 7;
    if operator > 4 || mmr && (kind != 0 || skip_enabled) {
        return Err(Error::syntax("invalid JBIG2 halftone flags"));
    }
    let grid = Grid {
        width: reader.u32()?,
        height: reader.u32()?,
        x: i64::from(reader.u32()?.cast_signed()),
        y: i64::from(reader.u32()?.cast_signed()),
        rx: i64::from(reader.u16()?),
        ry: i64::from(reader.u16()?),
    };
    let cells = u64::from(grid.width) * u64::from(grid.height);
    let count = dictionary.patterns.len();
    let bpp = (count - 1).bit_width();
    work.pixels(
        (u64::from(grid.width.max(1)) * u64::from(grid.height))
            .checked_mul(u64::from(bpp + 1))
            .ok_or_else(|| Error::limit("JBIG2 gray-scale work overflow"))?,
        limits,
    )?;
    let cell_count =
        usize::try_from(cells).map_err(|_| Error::limit("JBIG2 gray-scale cell count overflow"))?;
    let indices_bytes = cell_count
        .checked_mul(size_of::<usize>())
        .ok_or_else(|| Error::limit("JBIG2 gray-scale size overflow"))?;
    let (_, plane_bytes) = super::bitmap::layout(grid.width, grid.height, limits)?;
    let (_, output_bytes) = super::bitmap::layout(size.0, size.1, limits)?;
    let template = Template::prescribed(kind, None);
    let peak = indices_bytes
        .checked_add(plane_bytes.saturating_mul(3))
        .and_then(|n| n.checked_add(output_bytes))
        .and_then(|n| n.checked_add(if mmr { 0 } else { template.context_bytes() }))
        .ok_or_else(|| Error::limit("JBIG2 gray-scale size overflow"))?;
    if peak > limits.decoded_bytes {
        return Err(Error::limit("JBIG2 gray-scale buffers exceed byte budget"));
    }
    let pattern = &dictionary.patterns[0];
    let mut skip = Bitmap::new(grid.width, grid.height, false, limits)?;
    if skip_enabled {
        for m in 0..grid.height {
            for n in 0..grid.width {
                let (x, y) = grid.position(n, m);
                skip.set(
                    n,
                    m,
                    u8::from(
                        x + i64::from(pattern.width) <= 0
                            || x >= i64::from(size.0)
                            || y + i64::from(pattern.height) <= 0
                            || y >= i64::from(size.1),
                    ),
                );
            }
        }
    }
    let indices = gray(
        reader,
        &grid,
        &template,
        skip_enabled.then_some(&skip),
        bpp,
        mmr,
        limits,
    )?;
    let mut output = Bitmap::new(size.0, size.1, flags & 128 != 0, limits)?;
    work.operations(cell_count, limits)?;
    // Charge every candidate pattern, including fully clipped placements.
    work.pixels(
        cells
            .checked_mul(u64::from(pattern.width) * u64::from(pattern.height))
            .ok_or_else(|| Error::limit("JBIG2 halftone drawing work overflow"))?,
        limits,
    )?;
    for m in 0..grid.height {
        for n in 0..grid.width {
            let index = indices[m as usize * grid.width as usize + n as usize];
            let pattern = dictionary.patterns.get(index).ok_or_else(|| {
                Error::syntax("JBIG2 gray-scale index exceeds pattern dictionary")
            })?;
            let (x, y) = grid.position(n, m);
            output.combine_signed(pattern, x, y, operator);
        }
    }
    Ok(output)
}

/// Gray-coded bitplanes share arithmetic statistics, Annex C.5 and 7.4.5.2.
fn gray(
    reader: &mut Reader<'_>,
    grid: &Grid,
    template: &Template,
    skip: Option<&Bitmap>,
    bpp: u32,
    mmr: bool,
    limits: &Limits,
) -> Result<Vec<usize>> {
    let cell_count = usize::try_from(u64::from(grid.width) * u64::from(grid.height))
        .map_err(|_| Error::limit("JBIG2 gray-scale cell count overflow"))?;
    let base = match reader.position() {
        Position::Buffer(n) => n,
        _ => 0,
    };
    let data = reader.rest();
    let mut decoder = if !mmr && bpp != 0 {
        Some(Decoder::new(data, base)?)
    } else {
        None
    };
    let mut contexts = if mmr {
        Vec::new()
    } else {
        vec![Context::default(); template.context_count()]
    };
    let mut indices = vec![0usize; cell_count];
    let mut previous = Bitmap::new(grid.width, grid.height, false, limits)?;
    let mut offset = 0;
    for bit in (0..bpp).rev() {
        let mut plane = if mmr {
            let (plane, consumed) = mmr::decode_prefix(
                &data[offset..],
                grid.width,
                grid.height,
                limits,
                base + offset,
                true,
            )?;
            offset += consumed;
            plane
        } else {
            generic::decode_skipping(
                (grid.width, grid.height),
                template,
                false,
                decoder
                    .as_mut()
                    .ok_or_else(|| Error::syntax("missing JBIG2 arithmetic gray plane decoder"))?,
                &mut contexts,
                skip,
                limits,
            )?
        };
        for m in 0..grid.height {
            for n in 0..grid.width {
                let value = plane.pixel(i64::from(n), i64::from(m))
                    ^ previous.pixel(i64::from(n), i64::from(m));
                plane.set(n, m, value);
                indices[m as usize * grid.width as usize + n as usize] |= usize::from(value) << bit;
            }
        }
        previous = plane;
    }
    if (mmr || bpp == 0) && offset != data.len() {
        return Err(Error::syntax("trailing JBIG2 gray-scale data"));
    }
    Ok(indices)
}
