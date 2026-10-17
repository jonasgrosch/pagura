//! Part 1 markers, tile assembly and component reconstruction (Annexes A–G).

use pagura_core::{Error, Result};

use super::coding::{Configuration, Quantization, Style};
use super::io::{Budget, Reader, area, syntax, usize_from};
use super::packet::{self, Band};
use super::tier1;
use super::wavelet::{self, Plane};
use super::{JpxColor, JpxComponent, JpxImage, unsupported};

#[derive(Clone)]
struct Component {
    bits: u8,
    signed: bool,
    dx: u32,
    dy: u32,
}

struct Size {
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
    tile_width: u32,
    tile_height: u32,
    tile_x0: u32,
    tile_y0: u32,
    columns: u32,
    rows: u32,
    components: Vec<Component>,
}

fn read_size(reader: &mut Reader<'_>, budget: &mut Budget<'_>) -> Result<Size> {
    let capabilities = reader.u16()?;
    if capabilities > 2 {
        return Err(unsupported(
            "JPEG 2000 extended capabilities are not implemented",
            reader.position() - 2,
        ));
    }
    let x1 = reader.u32()?;
    let y1 = reader.u32()?;
    let x0 = reader.u32()?;
    let y0 = reader.u32()?;
    let tile_width = reader.u32()?;
    let tile_height = reader.u32()?;
    let tile_x0 = reader.u32()?;
    let tile_y0 = reader.u32()?;
    if x1 <= x0
        || y1 <= y0
        || tile_width == 0
        || tile_height == 0
        || tile_x0 > x0
        || tile_y0 > y0
        || tile_width <= x0 - tile_x0
        || tile_height <= y0 - tile_y0
        || [x1, y1, tile_width, tile_height]
            .iter()
            .any(|&v| v > i32::MAX as u32)
    {
        return Err(syntax(
            "invalid JPEG 2000 reference or tile grid",
            reader.position(),
        ));
    }
    if area(x1 - x0, y1 - y0)? > budget.limits.pixels {
        return Err(Error::limit("JPEG 2000 pixel budget exceeded"));
    }
    let columns = (x1 - tile_x0).div_ceil(tile_width);
    let rows = (y1 - tile_y0).div_ceil(tile_height);
    let tiles = area(columns, rows)?;
    if tiles > budget.limits.operations || tiles > budget.limits.objects {
        return Err(Error::limit("JPEG 2000 tile budget exceeded"));
    }
    if tiles > 65535 {
        return Err(syntax("too many JPEG 2000 tiles", reader.position()));
    }
    let count = usize::from(reader.u16()?);
    if count == 0 || count > 16384 {
        return Err(syntax(
            "invalid JPEG 2000 component count",
            reader.position(),
        ));
    }
    budget.charge::<Component>(count)?;
    let mut components = Vec::with_capacity(count);
    for _ in 0..count {
        let precision = reader.u8()?;
        let bits = (precision & 127) + 1;
        let dx = u32::from(reader.u8()?);
        let dy = u32::from(reader.u8()?);
        if bits > 38 || dx == 0 || dy == 0 {
            return Err(syntax(
                "invalid JPEG 2000 component precision or sampling",
                reader.position() - 3,
            ));
        }
        if bits > 16 {
            return Err(unsupported(
                "JPEG 2000 precision exceeds the 16-bit sample API",
                reader.position() - 3,
            ));
        }
        components.push(Component {
            bits,
            signed: precision & 128 != 0,
            dx,
            dy,
        });
    }
    reader.finish()?;
    Ok(Size {
        x0,
        y0,
        x1,
        y1,
        tile_width,
        tile_height,
        tile_x0,
        tile_y0,
        columns,
        rows,
        components,
    })
}

fn marker(reader: &mut Reader<'_>, budget: &mut Budget<'_>) -> Result<(u16, usize)> {
    budget.work(1)?;
    let position = reader.position();
    if reader.u8()? != 255 {
        return Err(syntax("missing JPEG 2000 marker prefix", position));
    }
    let mut code = reader.u8()?;
    while code == 255 {
        budget.work(1)?;
        code = reader.u8()?;
    }
    if code == 0 {
        return Err(syntax("invalid JPEG 2000 marker", position));
    }
    Ok((0xff00 | u16::from(code), position))
}

fn segment<'a>(reader: &mut Reader<'a>) -> Result<Reader<'a>> {
    let length = usize::from(reader.u16()?);
    if length < 2 {
        return Err(syntax(
            "invalid JPEG 2000 marker length",
            reader.position() - 2,
        ));
    }
    let base = reader.position();
    Ok(Reader::new(reader.take(length - 2)?, base))
}

struct Resolution {
    bounds: [u32; 4],
    bands: Vec<Band>,
}
struct TileComponent {
    resolutions: Vec<Resolution>,
}
struct Tile {
    config: Configuration,
    components: Vec<TileComponent>,
    packets: Vec<(usize, usize, usize)>, // layer, resolution, component
    next_packet: usize,
    next_part: u16,
    part_count: Option<u16>,
}

impl Tile {
    fn new(
        size: &Size,
        index: u32,
        config: Configuration,
        budget: &mut Budget<'_>,
    ) -> Result<Self> {
        config.validate()?;
        let column = index % size.columns;
        let row = index / size.columns;
        let tx0 = size.tile_x0 + column * size.tile_width;
        let ty0 = size.tile_y0 + row * size.tile_height;
        let tx1 = tx0.saturating_add(size.tile_width).min(size.x1);
        let ty1 = ty0.saturating_add(size.tile_height).min(size.y1);
        let tx0 = tx0.max(size.x0);
        let ty0 = ty0.max(size.y0);
        budget.charge::<TileComponent>(size.components.len())?;
        let mut components = Vec::with_capacity(size.components.len());
        for (component, style) in size.components.iter().zip(&config.styles) {
            let [cx0, cy0, cx1, cy1] = [
                tx0.div_ceil(component.dx),
                ty0.div_ceil(component.dy),
                tx1.div_ceil(component.dx),
                ty1.div_ceil(component.dy),
            ];
            budget.charge::<Resolution>(usize::from(style.levels) + 1)?;
            let mut resolutions = Vec::with_capacity(usize::from(style.levels) + 1);
            for resolution in 0..=style.levels {
                let shift = style.levels - resolution;
                let scale = 1_u64 << shift;
                let reduce = |v: u32| -> Result<u32> {
                    u32::try_from(u64::from(v).div_ceil(scale))
                        .map_err(|_| Error::limit("JPEG 2000 resolution coordinate overflow"))
                };
                let bounds = [reduce(cx0)?, reduce(cy0)?, reduce(cx1)?, reduce(cy1)?];
                let [x0, y0, x1, y1] = bounds;
                if x0 != x1
                    && y0 != y1
                    && (x0 >> 15 != (x1 - 1) >> 15 || y0 >> 15 != (y1 - 1) >> 15)
                {
                    return Err(unsupported(
                        "multiple JPEG 2000 precincts per resolution are not implemented",
                        0,
                    ));
                }
                let bands_count = if resolution == 0 { 1 } else { 3 };
                budget.charge::<Band>(bands_count)?;
                let mut bands = Vec::with_capacity(bands_count);
                if resolution == 0 {
                    bands.push(Band::new(
                        Plane::new(x0, y0, x1, y1, budget)?,
                        0,
                        style.block,
                        budget,
                    )?);
                } else {
                    for orientation in 1..=3 {
                        let high_x = orientation & 1 != 0;
                        let high_y = orientation & 2 != 0;
                        let bx0 = if high_x { x0 / 2 } else { x0.div_ceil(2) };
                        let bx1 = if high_x { x1 / 2 } else { x1.div_ceil(2) };
                        let by0 = if high_y { y0 / 2 } else { y0.div_ceil(2) };
                        let by1 = if high_y { y1 / 2 } else { y1.div_ceil(2) };
                        bands.push(Band::new(
                            Plane::new(bx0, by0, bx1, by1, budget)?,
                            orientation,
                            style.block,
                            budget,
                        )?);
                    }
                }
                resolutions.push(Resolution { bounds, bands });
            }
            components.push(TileComponent { resolutions });
        }
        let packets = packet_order(&components, &config, budget)?;
        Ok(Self {
            config,
            components,
            packets,
            next_packet: 0,
            next_part: 0,
            part_count: None,
        })
    }

    fn read_packets(&mut self, reader: &mut Reader<'_>, budget: &mut Budget<'_>) -> Result<()> {
        let mut sequence = 0_u16;
        while reader.remaining() != 0 {
            let &(layer, resolution, component) = self
                .packets
                .get(self.next_packet)
                .ok_or_else(|| syntax("extra JPEG 2000 packet data", reader.position()))?;
            packet::read(
                reader,
                &mut self.components[component].resolutions[resolution].bands,
                layer,
                self.config.markers,
                sequence,
                budget,
            )?;
            self.next_packet += 1;
            sequence = sequence.wrapping_add(1);
        }
        Ok(())
    }

    fn reconstruct(self, size: &Size, image: &mut JpxImage, budget: &mut Budget<'_>) -> Result<()> {
        if self.next_packet != self.packets.len()
            || self.part_count.is_some_and(|n| self.next_part != n)
        {
            return Err(syntax("incomplete JPEG 2000 tile or tile-parts", 0));
        }
        budget.charge::<Plane>(self.components.len())?;
        let mut planes = Vec::with_capacity(self.components.len());
        for ((component, style), (quant, source)) in self
            .components
            .into_iter()
            .zip(&self.config.styles)
            .zip(self.config.quants.iter().zip(&size.components))
        {
            planes.push(reconstruct_component(
                component,
                style,
                quant,
                source.bits,
                budget,
            )?);
        }
        if self.config.transform {
            component_transform(&mut planes, self.config.styles[0].reversible, budget)?;
        }
        for ((plane, source), output) in planes
            .into_iter()
            .zip(&size.components)
            .zip(&mut image.components)
        {
            let origin_x = size.x0.div_ceil(source.dx);
            let origin_y = size.y0.div_ceil(source.dy);
            let width = usize_from(output.width)?;
            let plane_width = usize_from(plane.width)?;
            budget.work(plane.values.len())?;
            let level = f64::from(1_u32 << (source.bits - 1));
            let maximum = f64::from((1_u32 << source.bits) - 1);
            for (index, value) in plane.values.into_iter().enumerate() {
                let x = usize_from(plane.x0 - origin_x)? + index % plane_width;
                let y = usize_from(plane.y0 - origin_y)? + index / plane_width;
                // Signed samples are exposed with the same unsigned offset as unsigned samples.
                let shifted = (value.round() + level).clamp(0.0, maximum);
                output.samples[y * width + x] = sample_u16(shifted);
            }
        }
        Ok(())
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the reconstructed sample is finite, rounded and clamped to 0..65535"
)]
fn sample_u16(value: f64) -> u16 {
    value as u16
}

// A single precinct per resolution collapses spatial progression to these
// bounded nested axes. Enumerating them directly keeps packet ordering linear.
fn packet_order(
    components: &[TileComponent],
    config: &Configuration,
    budget: &mut Budget<'_>,
) -> Result<Vec<(usize, usize, usize)>> {
    let resolutions = components
        .iter()
        .map(|c| c.resolutions.len())
        .max()
        .unwrap_or(0);
    let mut count = 0_usize;
    for resolution in components.iter().flat_map(|c| &c.resolutions) {
        let [x0, y0, x1, y1] = resolution.bounds;
        if x0 != x1 && y0 != y1 {
            count = count
                .checked_add(config.layers)
                .ok_or_else(|| Error::limit("JPEG 2000 packet count overflow"))?;
        }
    }
    let axes = match config.progression {
        1 => [resolutions, config.layers, components.len()],
        2 => [resolutions, components.len(), config.layers],
        3 | 4 => [components.len(), resolutions, config.layers],
        _ => [config.layers, resolutions, components.len()],
    };
    budget.work(
        axes[0]
            .checked_mul(axes[1])
            .and_then(|v| v.checked_mul(axes[2]))
            .ok_or_else(|| Error::limit("JPEG 2000 packet work overflow"))?,
    )?;
    budget.charge::<(usize, usize, usize)>(count)?;
    let mut packets = Vec::new();
    packets
        .try_reserve_exact(count)
        .map_err(|_| Error::limit("JPEG 2000 packet allocation failed"))?;
    for a in 0..axes[0] {
        for b in 0..axes[1] {
            for c in 0..axes[2] {
                let (layer, resolution, component) = match config.progression {
                    1 => (b, a, c),
                    2 => (c, a, b),
                    3 | 4 => (c, b, a),
                    _ => (a, b, c),
                };
                if let Some(r) = components[component].resolutions.get(resolution) {
                    let [x0, y0, x1, y1] = r.bounds;
                    if x0 != x1 && y0 != y1 {
                        packets.push((layer, resolution, component));
                    }
                }
            }
        }
    }
    Ok(packets)
}

fn reconstruct_component(
    component: TileComponent,
    style: &Style,
    quant: &Quantization,
    bits: u8,
    budget: &mut Budget<'_>,
) -> Result<Plane> {
    let mut low = None;
    let mut band_index = 0;
    for (resolution, mut r) in component.resolutions.into_iter().enumerate() {
        for band in &mut r.bands {
            let (exponent, mantissa) = if quant.kind == 1 {
                let (exp, mant) = quant.steps[0];
                let reduction = if resolution == 0 {
                    0
                } else {
                    usize::from(style.levels) - resolution
                };
                (
                    exp.checked_sub(
                        u8::try_from(reduction)
                            .map_err(|_| syntax("JPEG 2000 quantization exponent overflow", 0))?,
                    )
                    .ok_or_else(|| {
                        unsupported("JPEG 2000 derived exponent is below the supported range", 0)
                    })?,
                    mant,
                )
            } else {
                quant.steps[band_index]
            };
            let maximum_planes = exponent
                .checked_add(quant.guard)
                .and_then(|v| v.checked_sub(1))
                .ok_or_else(|| syntax("invalid JPEG 2000 quantization precision", 0))?;
            if maximum_planes > 30 {
                return Err(unsupported(
                    "JPEG 2000 coefficient precision exceeds 30 bit planes",
                    0,
                ));
            }
            let step = if style.reversible {
                1.0
            } else {
                (1.0 + f64::from(mantissa) / 2048.0)
                    * 2.0_f64.powi(i32::from(bits) - i32::from(exponent))
            };
            for block in &band.blocks {
                if block.passes == 0 {
                    continue;
                }
                let planes = maximum_planes
                    .checked_sub(block.zero_planes)
                    .filter(|&n| n != 0)
                    .ok_or_else(|| syntax("invalid JPEG 2000 zero bit-plane count", block.base))?;
                if block.passes != usize::from(planes) * 3 - 2 {
                    return Err(unsupported(
                        "JPEG 2000 partial bit-plane quality truncation is not implemented",
                        block.base,
                    ));
                }
                let parameters = tier1::Parameters {
                    width: block.width,
                    height: block.height,
                    orientation: band.orientation,
                    planes,
                    passes: block.passes,
                    reset: style.modes & 2 != 0,
                    segmentation_symbols: style.modes & 32 != 0,
                };
                let values = tier1::decode(&block.data, &parameters, block.base, budget)?;
                let width = usize_from(band.plane.width)?;
                for (index, value) in values.into_iter().enumerate() {
                    let coefficient = if style.reversible || value == 0 {
                        value as f64
                    } else {
                        value as f64 + if value < 0 { -0.5 } else { 0.5 }
                    };
                    band.plane.values
                        [(block.y + index / block.width) * width + block.x + index % block.width] =
                        coefficient * step;
                }
            }
            band_index += 1;
        }
        if resolution == 0 {
            low = r.bands.pop().map(|band| band.plane);
        } else {
            let previous = low
                .as_ref()
                .ok_or_else(|| syntax("missing JPEG 2000 low-pass subband", 0))?;
            low = Some(wavelet::synthesize(
                previous,
                [&r.bands[0].plane, &r.bands[1].plane, &r.bands[2].plane],
                r.bounds,
                style.reversible,
                budget,
            )?);
        }
    }
    low.ok_or_else(|| syntax("missing JPEG 2000 resolutions", 0))
}

fn component_transform(
    planes: &mut [Plane],
    reversible: bool,
    budget: &mut Budget<'_>,
) -> Result<()> {
    let (first, rest) = planes.split_at_mut(1);
    let (second, rest) = rest.split_at_mut(1);
    let a = &mut first[0];
    let b = &mut second[0];
    let c = &mut rest[0];
    if [b.width, c.width] != [a.width, a.width]
        || [b.height, c.height] != [a.height, a.height]
        || b.x0 != a.x0
        || c.x0 != a.x0
        || b.y0 != a.y0
        || c.y0 != a.y0
    {
        return Err(syntax(
            "JPEG 2000 transform components have different grids",
            0,
        ));
    }
    budget.work(
        a.values
            .len()
            .checked_mul(8)
            .ok_or_else(|| Error::limit("JPEG 2000 component transform work overflow"))?,
    )?;
    for ((a, b), c) in a.values.iter_mut().zip(&mut b.values).zip(&mut c.values) {
        let (y, cb, cr) = (*a, *b, *c);
        if reversible {
            let green = y - ((cb + cr) / 4.0).floor();
            *a = cr + green;
            *b = green;
            *c = cb + green;
        } else {
            *a = y + 1.402 * cr;
            *b = y - 0.344_13 * cb - 0.714_14 * cr;
            *c = y + 1.772 * cb;
        }
    }
    Ok(())
}

fn allocate_image(size: &Size, budget: &mut Budget<'_>) -> Result<JpxImage> {
    budget.charge::<JpxComponent>(size.components.len())?;
    let mut components = Vec::with_capacity(size.components.len());
    for c in &size.components {
        let width = size.x1.div_ceil(c.dx) - size.x0.div_ceil(c.dx);
        let height = size.y1.div_ceil(c.dy) - size.y0.div_ceil(c.dy);
        components.push(JpxComponent {
            bits: c.bits,
            signed: c.signed,
            width,
            height,
            samples: budget.filled(area(width, height)?, 0)?,
        });
    }
    Ok(JpxImage {
        width: size.x1 - size.x0,
        height: size.y1 - size.y0,
        components,
        color: JpxColor::Unspecified,
        alpha: None,
    })
}

pub(super) fn decode(data: &[u8], base: usize, budget: &mut Budget<'_>) -> Result<JpxImage> {
    let mut reader = Reader::new(data, base);
    let (size, config, mut next) = read_header(&mut reader, budget)?;
    let count = area(size.columns, size.rows)?;
    budget.charge::<Option<Tile>>(count)?;
    let mut tiles: Vec<Option<Tile>> = (0..count).map(|_| None).collect();
    let mut image = allocate_image(&size, budget)?;
    loop {
        let tile_start = next.1;
        let mut fields = segment(&mut reader)?;
        let index = usize::from(fields.u16()?);
        let length = usize_from(fields.u32()?)?;
        let part = u16::from(fields.u8()?);
        let parts = u16::from(fields.u8()?);
        fields.finish()?;
        if index >= count || parts != 0 && part >= parts {
            return Err(syntax("invalid JPEG 2000 tile-part index", tile_start));
        }
        let end = tile_part_end(data, base, tile_start, length, reader.position())?;
        let header_base = reader.position();
        let body = reader.take(end - header_base)?;
        let mut tile_reader = Reader::new(body, header_base);
        let mut local = if tiles[index].is_none() {
            Some(config.copy(budget)?)
        } else {
            None
        };
        loop {
            let (code, position) = marker(&mut tile_reader, budget)?;
            if code == 0xff93 {
                break;
            }
            if matches!(
                code,
                0xff4f | 0xff51 | 0xff55 | 0xff90 | 0xff91 | 0xff92 | 0xffd9
            ) {
                return Err(syntax("misplaced JPEG 2000 tile-header marker", position));
            }
            if let Some(local) = &mut local {
                local.marker(code, &mut segment(&mut tile_reader)?, budget)?;
            } else {
                match code {
                    0xff58 | 0xff64 => {
                        segment(&mut tile_reader)?;
                    }
                    _ => {
                        return Err(unsupported(
                            "JPEG 2000 coding changes after the first tile-part are not implemented",
                            position,
                        ));
                    }
                }
            }
        }
        if let Some(local) = local {
            tiles[index] = Some(Tile::new(
                &size,
                u32::try_from(index).map_err(|_| Error::limit("JPEG 2000 tile index overflow"))?,
                local,
                budget,
            )?);
        }
        let tile = tiles[index]
            .as_mut()
            .ok_or_else(|| syntax("missing JPEG 2000 tile", tile_start))?;
        if tile.next_part != part || parts != 0 && tile.part_count.is_some_and(|n| n != parts) {
            return Err(syntax("out-of-order JPEG 2000 tile-part", tile_start));
        }
        if parts != 0 {
            tile.part_count = Some(parts);
        }
        tile.next_part += 1;
        tile.read_packets(&mut tile_reader, budget)?;
        next = marker(&mut reader, budget)?;
        if next.0 == 0xffd9 {
            break;
        }
        if length == 0 || next.0 != 0xff90 {
            return Err(syntax("missing JPEG 2000 SOT or EOC", next.1));
        }
    }
    reader.finish()?;
    for tile in tiles {
        tile.ok_or_else(|| syntax("missing JPEG 2000 tile", base))?
            .reconstruct(&size, &mut image, budget)?;
    }
    Ok(image)
}

fn tile_part_end(
    data: &[u8],
    base: usize,
    start: usize,
    length: usize,
    position: usize,
) -> Result<usize> {
    if length == 0 {
        if data.get(data.len().saturating_sub(2)..) != Some(&[255, 217]) {
            return Err(syntax("unterminated JPEG 2000 final tile-part", start));
        }
        Ok(base + data.len() - 2)
    } else {
        start
            .checked_add(length)
            .filter(|&n| n >= position && n <= base + data.len())
            .ok_or_else(|| syntax("invalid JPEG 2000 tile-part length", start))
    }
}

fn read_header(
    reader: &mut Reader<'_>,
    budget: &mut Budget<'_>,
) -> Result<(Size, Configuration, (u16, usize))> {
    if marker(reader, budget)?.0 != 0xff4f || marker(reader, budget)?.0 != 0xff51 {
        return Err(syntax("missing JPEG 2000 SOC or SIZ", reader.base));
    }
    let size = read_size(&mut segment(reader)?, budget)?;
    let mut config = Configuration::new(size.components.len(), budget)?;
    let mut next = marker(reader, budget)?;
    while next.0 != 0xff90 {
        if next.0 == 0xffd9 {
            return Err(syntax("JPEG 2000 codestream has no tiles", next.1));
        }
        if matches!(next.0, 0xff4f | 0xff51 | 0xff58 | 0xff93 | 0xff91 | 0xff92) {
            return Err(syntax("misplaced JPEG 2000 main-header marker", next.1));
        }
        config.marker(next.0, &mut segment(reader)?, budget)?;
        next = marker(reader, budget)?;
    }
    config.validate()?;
    Ok((size, config, next))
}
