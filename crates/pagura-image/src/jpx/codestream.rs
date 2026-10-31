//! Tile assembly and reconstruction, T.800 Annex A, B.1–B.12, E.1 and G.1–G.4.

use pagura_core::{Error, Position, Result};

use super::coding::{Configuration, Quantization, Style};
use super::io::{Budget, Reader, area, syntax, usize_from};
use super::packed::{Fragments, Ppm};
use super::packet::{self, Band};
use super::progression::{self, Packet, Precinct};
use super::tier1;
use super::wavelet::{self, Plane};
use super::{ComponentGeometry, Decoded, JpxColor, JpxComponent, JpxImage, unsupported};

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

impl Size {
    fn validate_config(&self, config: &Configuration) -> Result<()> {
        config.validate()?;
        if config.transform
            && self.components[..3].iter().any(|c| {
                c.dx != self.components[0].dx
                    || c.dy != self.components[0].dy
                    || c.bits != self.components[0].bits
            })
        {
            return Err(syntax(
                "JPEG 2000 transform components have different sampling or precision",
                0,
            ));
        }
        if self
            .components
            .iter()
            .any(|c| !c.dx.is_power_of_two() || !c.dy.is_power_of_two())
            && (matches!(config.progression, 2 | 3) && config.progressions.is_empty()
                || config.progressions.iter().any(|p| matches!(p.order, 2 | 3)))
        {
            return Err(syntax(
                "JPEG 2000 spatial progression requires power-of-two sampling",
                0,
            ));
        }
        Ok(())
    }
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
    {
        return Err(syntax(
            "invalid JPEG 2000 reference or tile grid",
            reader.position(),
        ));
    }
    if [x1, y1, tile_width, tile_height]
        .iter()
        .any(|&v| v > i32::MAX as u32)
    {
        return Err(unsupported(
            "JPEG 2000 grid coordinates exceed supported arithmetic",
            reader.position(),
        ));
    }
    if area(x1 - x0, y1 - y0)? > budget.limits.pixels {
        return Err(Error::limit("JPEG 2000 pixel budget exceeded"));
    }
    let columns = (x1 - tile_x0).div_ceil(tile_width);
    let rows = (y1 - tile_y0).div_ceil(tile_height);
    let tiles = area(columns, rows)?;
    if tiles > budget.limits.decode_work || tiles > budget.limits.objects {
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
    loop {
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
        // A.1.3: the reserved no-parameter markers are skipped, without recursion.
        if (0x30..=0x3f).contains(&code) {
            continue;
        }
        return Ok((0xff00 | u16::from(code), position));
    }
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
    precincts: Vec<Precinct>,
}
struct TileComponent {
    resolutions: Vec<Resolution>,
}
struct Tile {
    config: Configuration,
    components: Vec<TileComponent>,
    packets: Vec<Packet>,
    all_packets: Vec<Packet>,
    visited: Vec<bool>,
    decoded_packets: usize,
    next_packet: usize,
    next_part: u16,
    part_count: Option<u16>,
    ignore_part_count: bool,
    local_poc: bool,
    packed_headers: Option<bool>,
}

impl Tile {
    fn new(
        size: &Size,
        index: u32,
        config: Configuration,
        budget: &mut Budget<'_>,
    ) -> Result<Self> {
        size.validate_config(&config)?;
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
                let precincts = progression::precincts(
                    bounds,
                    style.precincts[usize::from(resolution)],
                    scale,
                    [component.dx, component.dy],
                    [tx0, ty0],
                    budget,
                )?;
                let band_count = if resolution == 0 { 1 } else { 3 };
                budget.charge::<Band>(band_count)?;
                let mut bands = Vec::with_capacity(band_count);
                let precinct_exponents = style.precincts[usize::from(resolution)];
                let reduction = u8::from(resolution != 0);
                let block = [
                    style.block[0].min(precinct_exponents[0] - reduction),
                    style.block[1].min(precinct_exponents[1] - reduction),
                ];
                for orientation in if resolution == 0 { 0..=0 } else { 1..=3 } {
                    let [bx0, by0, bx1, by1] = progression::band_bounds(bounds, orientation);
                    bands.push(Band::new(
                        Plane::new(bx0, by0, bx1, by1),
                        orientation,
                        block,
                        &precincts,
                        budget,
                    )?);
                }
                resolutions.push(Resolution {
                    bounds,
                    bands,
                    precincts,
                });
            }
            components.push(TileComponent { resolutions });
        }
        let all_packets = packet_candidates(&components, config.layers, budget)?;
        let visited = budget.filled(all_packets.len(), false)?;
        let packets = progression::order(
            &all_packets,
            &config.progressions,
            config.progression,
            &visited,
            budget,
        )?;
        let local_poc = config.has_local_poc();
        Ok(Self {
            config,
            components,
            packets,
            all_packets,
            visited,
            decoded_packets: 0,
            next_packet: 0,
            next_part: 0,
            part_count: None,
            ignore_part_count: false,
            local_poc,
            packed_headers: None,
        })
    }

    fn read_packets(
        &mut self,
        reader: &mut Reader<'_>,
        mut packed: Option<&mut Reader<'_>>,
        tile_headers: bool,
        budget: &mut Budget<'_>,
    ) -> Result<()> {
        let mut sequence = u16::try_from(self.decoded_packets % 65536)
            .map_err(|_| Error::limit("JPEG 2000 packet sequence overflow"))?;
        while reader.remaining() != 0 || packed.as_ref().is_some_and(|r| r.remaining() != 0) {
            let packet = *self
                .packets
                .get(self.next_packet)
                .ok_or_else(|| syntax("extra JPEG 2000 packet data", reader.position()))?;
            let parameters = packet::Parameters {
                layer: packet.layer,
                precinct: packet.precinct,
                markers: self.config.markers,
                sequence,
                modes: self.config.styles[packet.component].modes,
                tile_headers,
            };
            packet::read(
                reader,
                packed.as_deref_mut(),
                &mut self.components[packet.component].resolutions[packet.resolution].bands,
                &parameters,
                budget,
            )?;
            self.visited[packet.id] = true;
            self.decoded_packets += 1;
            self.next_packet += 1;
            sequence = sequence.wrapping_add(1);
        }
        Ok(())
    }

    /// T.800 A.4.2: index order and byte lengths define the physical parts;
    /// inconsistent declarations may be ignored only through reported recovery.
    fn start_part(
        &mut self,
        part: &Part<'_>,
        position: usize,
        budget: &mut Budget<'_>,
    ) -> Result<()> {
        if self.next_part != part.number {
            return Err(syntax("out-of-order JPEG 2000 tile-part", position));
        }
        if !self.ignore_part_count && part.count != 0 {
            if part.number >= part.count || self.part_count.is_some_and(|n| n != part.count) {
                // Ordered, length-delimited parts identify the actual sequence
                // without guessing payload boundaries (T.800 A.4.2, Table A.5).
                if !budget.repaired_part_count {
                    budget.diagnostics.repair(
                        pagura_core::Diagnostic::new(
                            crate::tags::JPX_TILE_PART_COUNT,
                            "inconsistent JPEG 2000 tile-part count ignored (T.800 A.4.2)",
                        )
                        .at(Position::Buffer(position)),
                    )?;
                    budget.repaired_part_count = true;
                }
                self.ignore_part_count = true;
                self.part_count = None;
            } else {
                self.part_count = Some(part.count);
            }
        }
        self.next_part += 1;
        Ok(())
    }

    fn reconstruct(self, size: &Size, image: &mut JpxImage, budget: &mut Budget<'_>) -> Result<()> {
        if self.part_count.is_some_and(|n| self.next_part != n) {
            return Err(syntax("incomplete JPEG 2000 tile or tile-parts", 0));
        }
        budget.charge::<Plane>(self.components.len())?;
        let mut planes = Vec::with_capacity(self.components.len());
        for (((component, style), (quant, source)), roi) in self
            .components
            .into_iter()
            .zip(&self.config.styles)
            .zip(self.config.quants.iter().zip(&size.components))
            .zip(self.config.roi)
        {
            planes.push(reconstruct_component(
                component,
                style,
                quant,
                source.bits,
                roi,
                budget,
            )?);
        }
        let shared_luma = self.config.transform
            && component_transform(&mut planes, self.config.styles[0].reversible, budget)?;
        for ((index, source), output) in size
            .components
            .iter()
            .enumerate()
            .zip(&mut image.components)
        {
            let plane = &planes[if shared_luma && index < 3 { 0 } else { index }];
            let origin_x = size.x0.div_ceil(source.dx);
            let origin_y = size.y0.div_ceil(source.dy);
            let width = usize_from(output.width)?;
            let plane_width = usize_from(plane.width)?;
            budget.work(area(plane.width, plane.height)?)?;
            let level = f64::from(1_u32 << (source.bits - 1));
            let maximum = f64::from((1_u32 << source.bits) - 1);
            if plane.values.is_empty() {
                for y in 0..usize_from(plane.height)? {
                    let start = (usize_from(plane.y0 - origin_y)? + y) * width
                        + usize_from(plane.x0 - origin_x)?;
                    output.samples[start..start + plane_width].fill(sample_u16(level));
                }
                continue;
            }
            for (y, row) in plane.values.chunks_exact(plane_width).enumerate() {
                let start = (usize_from(plane.y0 - origin_y)? + y) * width
                    + usize_from(plane.x0 - origin_x)?;
                for (sample, value) in output.samples[start..start + plane_width]
                    .iter_mut()
                    .zip(row)
                {
                    // Signed samples use the same unsigned offset as unsigned samples.
                    *sample = sample_u16((value.round() + level).clamp(0.0, maximum));
                }
            }
        }
        for plane in &mut planes {
            budget.free(std::mem::take(&mut plane.values));
        }
        budget.free(planes);
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

fn packet_candidates(
    components: &[TileComponent],
    layers: usize,
    budget: &mut Budget<'_>,
) -> Result<Vec<Packet>> {
    let mut result = Vec::new();
    for (c, component) in components.iter().enumerate() {
        for (r, resolution) in component.resolutions.iter().enumerate() {
            for (p, precinct) in resolution.precincts.iter().enumerate() {
                for layer in 0..layers {
                    let id = result.len();
                    budget.push(
                        &mut result,
                        Packet {
                            layer,
                            resolution: r,
                            component: c,
                            precinct: p,
                            id,
                            position: precinct.position,
                        },
                    )?;
                }
            }
        }
    }
    Ok(result)
}

fn reconstruct_component(
    component: TileComponent,
    style: &Style,
    quant: &Quantization,
    bits: u8,
    roi: u8,
    budget: &mut Budget<'_>,
) -> Result<Plane> {
    let mut low = None;
    let mut band_index = 0;
    for (resolution, mut r) in component.resolutions.into_iter().enumerate() {
        for band in &mut r.bands {
            let (exponent, mantissa) = if quant.kind == 1 {
                let (exp, mant) = quant.steps[0];
                let reduction = if resolution == 0 { 0 } else { resolution - 1 };
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
            let maximum_planes = u16::from(exponent) + u16::from(quant.guard) + u16::from(roi);
            if maximum_planes == 0 {
                return Err(syntax("invalid JPEG 2000 quantization precision", 0));
            }
            let maximum_planes = maximum_planes - 1;
            if maximum_planes > 30 {
                return Err(unsupported(
                    "JPEG 2000 coefficient precision exceeds 30 bit planes",
                    0,
                ));
            }
            let maximum_planes = u8::try_from(maximum_planes)
                .map_err(|_| Error::limit("JPEG 2000 coefficient precision overflow"))?;
            let step =
                quantization_step(style.reversible, bits, band.orientation, exponent, mantissa);
            for block in band.partitions.iter().flat_map(|p| &p.blocks) {
                if block.passes == 0 {
                    continue;
                }
                let planes = maximum_planes
                    .checked_sub(block.zero_planes)
                    .filter(|&n| n != 0)
                    .ok_or_else(|| syntax("invalid JPEG 2000 zero bit-plane count", block.base))?;
                let parameters = tier1::Parameters {
                    width: block.width,
                    height: block.height,
                    orientation: band.orientation,
                    planes,
                    passes: block.passes,
                    modes: style.modes,
                };
                let values = tier1::decode(&block.segments, &parameters, budget)?;
                let width = usize_from(band.plane.width)?;
                for (y, row) in values.chunks_exact(block.width).enumerate() {
                    let start = (block.y + y) * width + block.x;
                    let mut nonzero = false;
                    for (x, value) in row.iter().enumerate() {
                        let coefficient = value.reconstruct(roi, style.reversible);
                        if coefficient == 0.0 {
                            continue;
                        }
                        // The first nonzero coefficient of the plane allocates it.
                        if !nonzero {
                            band.plane.materialize(budget)?;
                            nonzero = true;
                        }
                        band.plane.values[start + x] = coefficient * step;
                    }
                    if nonzero {
                        band.plane.include_rows(block.y + y..block.y + y + 1);
                    }
                }
                budget.free(values);
            }
            band_index += 1;
        }
        if resolution == 0 {
            low = r.bands.pop().map(|band| band.plane);
        } else {
            let previous = low
                .take()
                .ok_or_else(|| syntax("missing JPEG 2000 low-pass subband", 0))?;
            low = Some(wavelet::synthesize(
                &previous,
                [&r.bands[0].plane, &r.bands[1].plane, &r.bands[2].plane],
                r.bounds,
                style.reversible,
                budget,
            )?);
            // The next resolution's plane replaces this one's subbands.
            budget.free(previous.values);
            for band in r.bands {
                budget.free(band.plane.values);
            }
        }
    }
    low.ok_or_else(|| syntax("missing JPEG 2000 resolutions", 0))
}

fn quantization_step(
    reversible: bool,
    bits: u8,
    orientation: u8,
    exponent: u8,
    mantissa: u16,
) -> f64 {
    // T.800 E-3/E-4, Table E.1: HL/LH gain 2; HH gain 4.
    if reversible {
        return 1.0;
    }
    let gain = u8::from(orientation & 1 != 0) + u8::from(orientation & 2 != 0);
    (1.0 + f64::from(mantissa) / 2048.0)
        * 2.0_f64.powi(i32::from(bits) + i32::from(gain) - i32::from(exponent))
}

fn component_transform(
    planes: &mut [Plane],
    reversible: bool,
    budget: &mut Budget<'_>,
) -> Result<bool> {
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
    // G.2/G.3: zero chroma makes all three transformed channels equal to Y.
    // Keep one luma plane and let the output conversion sample it three times.
    if b.values.is_empty() && c.values.is_empty() {
        return Ok(true);
    }
    let cb_zero = b.values.is_empty();
    let cr_zero = c.values.is_empty();
    a.materialize(budget)?;
    b.materialize(budget)?;
    c.materialize(budget)?;
    budget.work(
        a.values
            .len()
            .checked_mul(if cb_zero || cr_zero {
                if reversible { 6 } else { 4 }
            } else {
                8
            })
            .ok_or_else(|| Error::limit("JPEG 2000 component transform work overflow"))?,
    )?;
    for ((a, b), c) in a.values.iter_mut().zip(&mut b.values).zip(&mut c.values) {
        let (y, cb, cr) = (*a, *b, *c);
        if reversible {
            let sum = if cb_zero {
                cr
            } else if cr_zero {
                cb
            } else {
                cb + cr
            };
            let green = y - (sum / 4.0).floor();
            *a = if cr_zero { green } else { cr + green };
            *b = green;
            *c = if cb_zero { green } else { cb + green };
        } else {
            *a = if cr_zero { y } else { y + 1.402 * cr };
            *b = if cb_zero { y } else { y - 0.344_13 * cb };
            if !cr_zero {
                *b -= 0.714_14 * cr;
            }
            *c = if cb_zero { y } else { y + 1.772 * cb };
        }
    }
    let start = [&*a, &*b, &*c]
        .iter()
        .filter(|p| !p.rows.is_empty())
        .map(|p| p.rows.start)
        .min()
        .unwrap_or(0);
    let end = a.rows.end.max(b.rows.end).max(c.rows.end);
    for plane in [a, b, c] {
        plane.rows = start..end;
    }
    Ok(false)
}

/// The image a codestream declares, with its components' precisions and
/// placement but no samples: what [`decode`] returns before reconstruction,
/// after the same main-header validation.
pub(super) fn describe(data: &[u8], base: usize, budget: &mut Budget<'_>) -> Result<Decoded> {
    let mut reader = Reader::new(data, base);
    let size = read_header(&mut reader, budget)?.size;
    allocate(&size, budget, false)
}

fn allocate_image(size: &Size, budget: &mut Budget<'_>) -> Result<Decoded> {
    allocate(size, budget, true)
}

fn allocate(size: &Size, budget: &mut Budget<'_>, samples: bool) -> Result<Decoded> {
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
            samples: if samples {
                budget.samples(area(width, height)?, 0)?
            } else {
                Vec::new()
            },
        });
    }
    let mut geometry = budget.filled(
        size.components.len(),
        ComponentGeometry {
            step: [1, 1],
            origin: [0, 0],
        },
    )?;
    for (output, source) in geometry.iter_mut().zip(&size.components) {
        *output = ComponentGeometry {
            step: [source.dx, source.dy],
            origin: [
                size.x0.div_ceil(source.dx) * source.dx,
                size.y0.div_ceil(source.dy) * source.dy,
            ],
        };
    }
    Ok(Decoded {
        reference_origin: [size.x0, size.y0],
        component_geometry: geometry,
        opacity: None,
        image: JpxImage {
            width: size.x1 - size.x0,
            height: size.y1 - size.y0,
            components,
            color: JpxColor::Unspecified,
            alpha: None,
        },
    })
}

struct MainHeader {
    size: Size,
    config: Configuration,
    next: (u16, usize),
    ppm: Ppm,
}

pub(super) fn decode(data: &[u8], base: usize, budget: &mut Budget<'_>) -> Result<Decoded> {
    let mut reader = Reader::new(data, base);
    let header = read_header(&mut reader, budget)?;
    let MainHeader {
        size,
        config,
        mut next,
        mut ppm,
    } = header;
    let count = area(size.columns, size.rows)?;
    budget.charge::<Option<Tile>>(count)?;
    let mut tiles: Vec<Option<Tile>> = (0..count).map(|_| None).collect();
    let mut image = allocate_image(&size, budget)?;
    loop {
        let mut part = read_part(&mut reader, data, next.1, count)?;
        let mut local = if tiles[part.index].is_none() {
            Some(config.copy(budget)?)
        } else {
            None
        };
        let ppt = tile_header(&mut part.reader, &mut local, &mut tiles[part.index], budget)?;
        if let Some(local) = local {
            tiles[part.index] = Some(Tile::new(
                &size,
                u32::try_from(part.index)
                    .map_err(|_| Error::limit("JPEG 2000 tile index overflow"))?,
                local,
                budget,
            )?);
        }
        let tile = tiles[part.index]
            .as_mut()
            .ok_or_else(|| syntax("missing JPEG 2000 tile", next.1))?;
        tile.start_part(&part, next.1, budget)?;
        if ppm.fragments.present() && ppt.present() {
            return Err(syntax("simultaneous JPEG 2000 PPM and PPT", next.1));
        }
        let mut packed = if ppm.fragments.present() {
            Some(ppm.next()?)
        } else {
            ppt.reader()
        };
        let uses_packed = packed.is_some();
        if part.reader.remaining() != 0 || packed.as_ref().is_some_and(|p| p.remaining() != 0) {
            if tile
                .packed_headers
                .is_some_and(|previous| previous != uses_packed)
            {
                if tile.packed_headers == Some(true) {
                    return Err(unsupported(
                        "JPEG 2000 PPT headers preceding a later tile-part are not implemented",
                        next.1,
                    ));
                }
                return Err(syntax(
                    "mixed inline and packed JPEG 2000 tile headers",
                    next.1,
                ));
            }
            tile.packed_headers = Some(uses_packed);
        }
        tile.read_packets(&mut part.reader, packed.as_mut(), ppt.present(), budget)?;
        next = marker(&mut reader, budget)?;
        if next.0 == 0xffd9 {
            break;
        }
        if part.length == 0 || next.0 != 0xff90 {
            return Err(syntax("missing JPEG 2000 SOT or EOC", next.1));
        }
    }
    reader.finish()?;
    ppm.finish()?;
    for tile in tiles {
        tile.ok_or_else(|| syntax("missing JPEG 2000 tile", base))?
            .reconstruct(&size, &mut image.image, budget)?;
    }
    Ok(image)
}

struct Part<'a> {
    index: usize,
    number: u16,
    count: u16,
    length: usize,
    reader: Reader<'a>,
}

fn read_part<'a>(
    reader: &mut Reader<'a>,
    data: &'a [u8],
    start: usize,
    tiles: usize,
) -> Result<Part<'a>> {
    let mut fields = segment(reader)?;
    let index = usize::from(fields.u16()?);
    let length = usize_from(fields.u32()?)?;
    let number = u16::from(fields.u8()?);
    let count = u16::from(fields.u8()?);
    fields.finish()?;
    if index >= tiles || number == 255 {
        return Err(syntax("invalid JPEG 2000 tile-part index", start));
    }
    let end = tile_part_end(data, reader.base, start, length, reader.position())?;
    let base = reader.position();
    Ok(Part {
        index,
        number,
        count,
        length,
        reader: Reader::new(reader.take(end - base)?, base),
    })
}

fn tile_header(
    reader: &mut Reader<'_>,
    local: &mut Option<Configuration>,
    tile: &mut Option<Tile>,
    budget: &mut Budget<'_>,
) -> Result<Fragments> {
    let mut ppt = Fragments::default();
    let mut seen_poc = false;
    loop {
        let (code, position) = marker(reader, budget)?;
        if code == 0xff93 {
            break;
        }
        if matches!(
            code,
            0xff4f | 0xff51 | 0xff55 | 0xff60 | 0xff90 | 0xff91 | 0xff92 | 0xffd9
        ) {
            return Err(syntax("misplaced JPEG 2000 tile-header marker", position));
        }
        if code == 0xff5f {
            if seen_poc {
                return Err(syntax("repeated JPEG 2000 POC in one header", position));
            }
            seen_poc = true;
        }
        let mut fields = segment(reader)?;
        if code == 0xff61 {
            ppt.push(&mut fields, budget)?;
        } else if let Some(local) = local {
            local.marker(code, &mut fields, budget)?;
        } else {
            let tile = tile
                .as_mut()
                .ok_or_else(|| syntax("missing JPEG 2000 tile", position))?;
            match code {
                0xff5f => {
                    if !tile.local_poc {
                        return Err(syntax(
                            "JPEG 2000 tile POC missing from first tile-part",
                            position,
                        ));
                    }
                    let entries = progression::changes(
                        &mut fields,
                        tile.config.styles.len(),
                        tile.config.layers,
                        budget,
                    )?;
                    for entry in entries {
                        budget.push(&mut tile.config.progressions, entry)?;
                    }
                    tile.packets = progression::order(
                        &tile.all_packets,
                        &tile.config.progressions,
                        tile.config.progression,
                        &tile.visited,
                        budget,
                    )?;
                    tile.next_packet = 0;
                }
                0xff58 | 0xff64 => tile.config.marker(code, &mut fields, budget)?,
                0xff52 | 0xff53 | 0xff5c | 0xff5d | 0xff5e => {
                    return Err(syntax(
                        "JPEG 2000 coding marker after the first tile-part",
                        position,
                    ));
                }
                _ => {
                    return Err(unsupported(
                        "JPEG 2000 tile-header extension is not implemented",
                        position,
                    ));
                }
            }
        }
    }
    Ok(ppt)
}

fn tile_part_end(
    data: &[u8],
    base: usize,
    start: usize,
    length: usize,
    position: usize,
) -> Result<usize> {
    if length == 0 {
        // A final tile-part (Psot 0) runs to the EOC marker, which must come
        // after its own SOT segment (T.800 A.4.2).
        if data.get(data.len().saturating_sub(2)..) != Some(&[255, 217]) {
            return Err(syntax("unterminated JPEG 2000 final tile-part", start));
        }
        let end = base + data.len() - 2;
        if end < position {
            return Err(syntax("JPEG 2000 final tile-part overlaps EOC", start));
        }
        Ok(end)
    } else {
        start
            .checked_add(length)
            .filter(|&n| n >= position && n <= base + data.len())
            .ok_or_else(|| syntax("invalid JPEG 2000 tile-part length", start))
    }
}

fn read_header(reader: &mut Reader<'_>, budget: &mut Budget<'_>) -> Result<MainHeader> {
    if marker(reader, budget)?.0 != 0xff4f || marker(reader, budget)?.0 != 0xff51 {
        return Err(syntax("missing JPEG 2000 SOC or SIZ", reader.base));
    }
    let size = read_size(&mut segment(reader)?, budget)?;
    let mut config = Configuration::new(size.components.len(), budget)?;
    let mut ppm = Ppm::default();
    let mut next = marker(reader, budget)?;
    while next.0 != 0xff90 {
        if next.0 == 0xffd9 {
            return Err(syntax("JPEG 2000 codestream has no tiles", next.1));
        }
        if matches!(
            next.0,
            0xff4f | 0xff51 | 0xff58 | 0xff61 | 0xff93 | 0xff91 | 0xff92
        ) {
            return Err(syntax("misplaced JPEG 2000 main-header marker", next.1));
        }
        if next.0 == 0xff60 {
            let mut fields = segment(reader)?;
            if fields.remaining() < 5 {
                return Err(syntax("short JPEG 2000 PPM marker", next.1));
            }
            ppm.fragments.push(&mut fields, budget)?;
        } else {
            config.marker(next.0, &mut segment(reader)?, budget)?;
        }
        next = marker(reader, budget)?;
    }
    config.validate()?;
    Ok(MainHeader {
        size,
        config,
        next,
        ppm,
    })
}

#[cfg(test)]
mod tests {
    use super::{Budget, Plane, component_transform, sample_u16};
    use pagura_core::Limits;

    #[test]
    fn zero_chroma_uses_the_rct_and_ict_identities() -> pagura_core::Result<()> {
        // G.2/G.3 with Y=10 and either Cb=4 or Cr=4, then nearest rounding.
        for (reversible, cb, cr, expected) in [
            (true, 0.0, 4.0, [13, 9, 9]),
            (true, 4.0, 0.0, [9, 9, 13]),
            (false, 0.0, 4.0, [16, 7, 10]),
            (false, 4.0, 0.0, [10, 9, 17]),
        ] {
            let limits = Limits::default();
            let mut budget = Budget::new(&limits);
            let mut planes = Vec::new();
            for value in [10.0, cb, cr] {
                let mut plane = Plane::new(1, 3, 2, 4);
                if value != 0.0 {
                    plane.materialize(&mut budget)?;
                    plane.values[0] = value;
                    plane.include_rows(0..1);
                }
                planes.push(plane);
            }
            assert!(!component_transform(&mut planes, reversible, &mut budget)?);
            for (plane, expected) in planes.iter().zip(expected) {
                assert_eq!(sample_u16(plane.values[0].round()), expected);
                assert_eq!(plane.rows, 0..1);
            }
        }
        Ok(())
    }
}
