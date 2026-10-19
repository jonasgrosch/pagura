//! Tile assembly and reconstruction, T.800 Annex A, B.1–B.12, E.1 and G.1–G.4.

use pagura_core::bytes::Cursor;
use pagura_core::convert::u32_to_usize;
use pagura_core::{Diagnostic, Diagnostics, Error, Position, Result, ResultExt};

use crate::budget::Budget;

use super::coding::{Configuration, Quantization, QuantizationStyle, Style};
use super::marker::Marker;
use super::packed::{Fragments, Headers, Ppm};
use super::packet::{self, Band, CodeBlock};
use super::progression::{self, Packet, Precinct};
use super::segments;
use super::tier1;
use super::wavelet::{self, Plane};
use super::{
    ComponentGeometry, Decoded, JpxColor, JpxComponent, JpxImage, area, finish, syntax, unsupported,
};

/// The most tiles a codestream can have: tile indices `Isot` run from 0 to
/// 65534 (T.800 A.4.2, Table A.5).
const MAX_TILES: usize = 65_535;

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
    /// Checks a tile's coding configuration, which takes effect at `position`.
    fn validate_config(&self, config: &Configuration, position: Position) -> Result<()> {
        config.validate(position)?;
        if config.transform
            && self.components[..3].iter().any(|c| {
                c.dx != self.components[0].dx
                    || c.dy != self.components[0].dy
                    || c.bits != self.components[0].bits
            })
        {
            return Err(syntax(
                "JPEG 2000 transform components have different sampling or precision",
                position,
            ));
        }
        if self
            .components
            .iter()
            .any(|c| !c.dx.is_power_of_two() || !c.dy.is_power_of_two())
            && (config.progression.is_spatial() && config.progressions.is_empty()
                || config.progressions.iter().any(|p| p.order.is_spatial()))
        {
            return Err(syntax(
                "JPEG 2000 spatial progression requires power-of-two sampling",
                position,
            ));
        }
        Ok(())
    }
}

fn read_size(reader: &mut Cursor<'_>, pixels: usize, budget: &mut Budget) -> Result<Size> {
    let start = reader.position();
    let capabilities = reader.u16()?;
    if capabilities > 2 {
        return Err(unsupported(
            "JPEG 2000 extended capabilities are not implemented",
            start,
        ));
    }
    let grid = reader.position();
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
        return Err(syntax("invalid JPEG 2000 reference or tile grid", grid));
    }
    if [x1, y1, tile_width, tile_height]
        .iter()
        .any(|&v| v > i32::MAX.unsigned_abs())
    {
        return Err(unsupported(
            "JPEG 2000 grid coordinates exceed supported arithmetic",
            grid,
        ));
    }
    if area(x1 - x0, y1 - y0)? > pixels {
        return Err(Error::limit("JPEG 2000 pixel budget exceeded").at(grid));
    }
    let columns = (x1 - tile_x0).div_ceil(tile_width);
    let rows = (y1 - tile_y0).div_ceil(tile_height);
    let tiles = area(columns, rows)?;
    if tiles > MAX_TILES {
        return Err(syntax("too many JPEG 2000 tiles", grid));
    }
    budget.work(tiles).at(grid)?;
    let count = usize::from(reader.u16()?);
    if count == 0 || count > 16384 {
        return Err(reader.error("invalid JPEG 2000 component count"));
    }
    let mut components = budget.with_capacity(count).at(start)?;
    for _ in 0..count {
        let at = reader.position();
        let precision = reader.u8()?;
        let bits = (precision & 127) + 1;
        let dx = u32::from(reader.u8()?);
        let dy = u32::from(reader.u8()?);
        if bits > 38 || dx == 0 || dy == 0 {
            return Err(syntax(
                "invalid JPEG 2000 component precision or sampling",
                at,
            ));
        }
        if bits > 16 {
            return Err(unsupported(
                "JPEG 2000 precision exceeds the 16-bit sample API",
                at,
            ));
        }
        components.push(Component {
            bits,
            signed: precision & 128 != 0,
            dx,
            dy,
        });
    }
    finish(reader)?;
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

/// A marker found in the codestream.
#[derive(Clone, Copy)]
struct Found {
    marker: Marker,
    /// The offset of its first byte in the codestream.
    offset: usize,
    position: Position,
}

fn marker(reader: &mut Cursor<'_>, budget: &mut Budget) -> Result<Found> {
    loop {
        let offset = reader.offset();
        let position = reader.position();
        budget.work(1).at(position)?;
        if reader.u8()? != 255 {
            return Err(syntax("missing JPEG 2000 marker prefix", position));
        }
        let mut code = reader.u8()?;
        while code == 255 {
            budget.work(1).at(position)?;
            code = reader.u8()?;
        }
        if code == 0 {
            return Err(syntax("invalid JPEG 2000 marker", position));
        }
        let marker = Marker::new(code);
        if marker.is_reserved() {
            continue;
        }
        return Ok(Found {
            marker,
            offset,
            position,
        });
    }
}

/// The parameters of a marker segment, after its length field.
fn segment<'a>(reader: &mut Cursor<'a>) -> Result<Cursor<'a>> {
    let position = reader.position();
    let length = usize::from(reader.u16()?)
        .checked_sub(2)
        .ok_or_else(|| syntax("invalid JPEG 2000 marker length", position))?;
    reader.take_cursor(length)
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
    /// Whether an inconsistent declared tile-part count was set aside, so
    /// that order and lengths alone delimit the tile-parts.
    ignore_part_count: bool,
    local_poc: bool,
    packed_headers: Option<bool>,
    /// Where the tile's first tile-part starts.
    start: Position,
}

impl Tile {
    fn new(
        size: &Size,
        index: u32,
        config: Configuration,
        start: Position,
        budget: &mut Budget,
    ) -> Result<Self> {
        size.validate_config(&config, start)?;
        let column = index % size.columns;
        let row = index / size.columns;
        let tx0 = size.tile_x0 + column * size.tile_width;
        let ty0 = size.tile_y0 + row * size.tile_height;
        let tx1 = tx0.saturating_add(size.tile_width).min(size.x1);
        let ty1 = ty0.saturating_add(size.tile_height).min(size.y1);
        let tx0 = tx0.max(size.x0);
        let ty0 = ty0.max(size.y0);
        let mut components = budget.with_capacity(size.components.len())?;
        for (component, style) in size.components.iter().zip(&config.styles) {
            let [cx0, cy0, cx1, cy1] = [
                tx0.div_ceil(component.dx),
                ty0.div_ceil(component.dy),
                tx1.div_ceil(component.dx),
                ty1.div_ceil(component.dy),
            ];
            let mut resolutions = budget.with_capacity(usize::from(style.levels) + 1)?;
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
                let mut bands = budget.with_capacity(band_count)?;
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
            start,
        })
    }

    fn read_packets(
        &mut self,
        reader: &mut Cursor<'_>,
        mut packed: Option<&mut Headers<'_>>,
        tile_headers: bool,
        budget: &mut Budget,
    ) -> Result<()> {
        let mut sequence = u16::try_from(self.decoded_packets % 65536)
            .map_err(|_| Error::limit("JPEG 2000 packet sequence overflow"))?;
        while !reader.at_end() || packed.as_ref().is_some_and(|r| r.remaining() != 0) {
            let packet = *self
                .packets
                .get(self.next_packet)
                .ok_or_else(|| reader.error("extra JPEG 2000 packet data"))?;
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

    /// Checks that `part`, which starts at `at`, is the tile's next
    /// tile-part (T.800 A.4.2). An inconsistent declared count of tile-parts
    /// is set aside only as a reported repair, once per codestream.
    fn start_part(
        &mut self,
        part: &Part<'_>,
        at: Position,
        stream: &mut Stream<'_, '_>,
    ) -> Result<()> {
        if self.next_part != part.number {
            return Err(syntax("out-of-order JPEG 2000 tile-part", at));
        }
        if !self.ignore_part_count && part.count != 0 {
            if part.number >= part.count || self.part_count.is_some_and(|n| n != part.count) {
                if !stream.part_count_reported {
                    stream.diagnostics.repair(
                        Diagnostic::new(
                            crate::repairs::JPX_TILE_PART_COUNT,
                            "inconsistent JPEG 2000 tile-part count ignored (T.800 A.4.2)",
                        )
                        .at(at),
                    )?;
                    stream.part_count_reported = true;
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

    /// Reconstructs the tile's samples into `image`. Errors without a
    /// position of their own are placed at the tile's first tile-part.
    fn reconstruct(self, size: &Size, image: &mut JpxImage, budget: &mut Budget) -> Result<()> {
        let start = self.start;
        self.reconstruct_at(size, image, budget).at(start)
    }

    fn reconstruct_at(self, size: &Size, image: &mut JpxImage, budget: &mut Budget) -> Result<()> {
        if self.part_count.is_some_and(|n| self.next_part != n) {
            return Err(Error::syntax("incomplete JPEG 2000 tile or tile-parts"));
        }
        let mut planes = budget.with_capacity(self.components.len())?;
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
        // Configuration::validate requires three components for a transform.
        let shared_luma = self.config.transform
            && component_transform(&mut planes, self.config.styles[0].reversible, budget)?;
        for ((index, source), output) in size
            .components
            .iter()
            .enumerate()
            .zip(&mut image.components)
        {
            // There is a plane per component; the first stands for three
            // when the transform left them equal.
            let plane = &planes[if shared_luma && index < 3 { 0 } else { index }];
            write_plane(plane, source, [size.x0, size.y0], output, budget)?;
        }
        for plane in planes {
            plane.release(budget);
        }
        Ok(())
    }
}

/// Writes a tile-component's plane into the component's samples, adding the
/// level shift (T.800 G.1.2) and clamping to the sample range.
fn write_plane(
    plane: &Plane,
    source: &Component,
    image_origin: [u32; 2],
    output: &mut JpxComponent,
    budget: &mut Budget,
) -> Result<()> {
    let origin_x = image_origin[0].div_ceil(source.dx);
    let origin_y = image_origin[1].div_ceil(source.dy);
    let width = u32_to_usize(output.width);
    let plane_width = u32_to_usize(plane.width);
    budget.work(area(plane.width, plane.height)?)?;
    let level = f64::from(1_u32 << (source.bits - 1));
    let maximum = f64::from((1_u32 << source.bits) - 1);
    for y in 0..u32_to_usize(plane.height) {
        let start =
            (u32_to_usize(plane.y0 - origin_y) + y) * width + u32_to_usize(plane.x0 - origin_x);
        // The tile-component lies inside the image component.
        let samples = output.samples.get_mut(start..start + plane_width);
        debug_assert!(samples.is_some(), "tile row {y} lies outside the image");
        let Some(samples) = samples else {
            continue;
        };
        // An empty plane is zero.
        let row = plane
            .values
            .get(y * plane_width..(y + 1) * plane_width)
            .filter(|_| !plane.values.is_empty());
        if let Some(row) = row {
            for (sample, value) in samples.iter_mut().zip(row) {
                // Signed samples use the same unsigned offset as unsigned samples.
                *sample = sample_u16((value.round() + level).clamp(0.0, maximum));
            }
        } else {
            samples.fill(sample_u16(level));
        }
    }
    Ok(())
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
    budget: &mut Budget,
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
    budget: &mut Budget,
) -> Result<Plane> {
    let mut low: Option<Plane> = None;
    let mut band_index = 0;
    // Style::levels is at most 32 (T.800 A.6.1), so resolutions count in a u8.
    for (resolution, mut r) in (0_u8..).zip(component.resolutions) {
        for band in &mut r.bands {
            let coding = band_coding(quant, style, bits, roi, band_index, resolution, band)?;
            for block in band.partitions.iter().flat_map(|p| &p.blocks) {
                decode_block(&mut band.plane, block, band.orientation, &coding, budget)?;
            }
            band_index += 1;
        }
        low = Some(match low {
            // Resolution 0 holds the LL band alone (T.800 B.5).
            None => r
                .bands
                .pop()
                .map_or_else(|| Plane::new(0, 0, 0, 0), |band| band.plane),
            Some(previous) => {
                // Tile::new gives every higher resolution its HL, LH and HH bands.
                let synthesized = wavelet::synthesize(
                    &previous,
                    [&r.bands[0].plane, &r.bands[1].plane, &r.bands[2].plane],
                    r.bounds,
                    style.reversible,
                    budget,
                )?;
                // The synthesized resolution replaces its inputs.
                previous.release(budget);
                for band in r.bands {
                    band.plane.release(budget);
                }
                synthesized
            }
        });
    }
    // Every tile-component has resolution 0 (T.800 B.5).
    debug_assert!(low.is_some(), "a tile-component has no resolutions");
    Ok(low.unwrap_or_else(|| Plane::new(0, 0, 0, 0)))
}

/// How the code-blocks of one subband are decoded and dequantized.
struct BandCoding {
    /// The most bit planes a coefficient has, `Mb` (T.800 E-2).
    planes: u8,
    /// The quantization step, one for reversible coding.
    step: f64,
    roi: u8,
    reversible: bool,
    modes: u8,
}

/// The coding of subband `index`, of resolution `resolution`.
fn band_coding(
    quant: &Quantization,
    style: &Style,
    bits: u8,
    roi: u8,
    index: usize,
    resolution: u8,
    band: &Band,
) -> Result<BandCoding> {
    // Configuration::validate requires a step per subband, or the one step
    // of derived quantization.
    let (exponent, mantissa) = if quant.style == QuantizationStyle::Derived {
        let (exponent, mantissa) = quant.steps[0];
        // T.800 E-5: the exponent falls by one per decomposition level.
        let exponent =
            exponent
                .checked_sub(resolution.saturating_sub(1))
                .ok_or(Error::unsupported(
                    super::UNSUPPORTED,
                    "JPEG 2000 derived exponent is below the supported range",
                ))?;
        (exponent, mantissa)
    } else {
        quant.steps[index]
    };
    let planes = (u16::from(exponent) + u16::from(quant.guard) + u16::from(roi))
        .checked_sub(1)
        .ok_or(Error::syntax("invalid JPEG 2000 quantization precision"))?;
    let planes = u8::try_from(planes)
        .ok()
        .filter(|&planes| planes <= segments::MAX_PLANES)
        .ok_or(Error::unsupported(
            super::UNSUPPORTED,
            "JPEG 2000 coefficient precision exceeds 30 bit planes",
        ))?;
    Ok(BandCoding {
        planes,
        step: quantization_step(style.reversible, bits, band.orientation, exponent, mantissa),
        roi,
        reversible: style.reversible,
        modes: style.modes,
    })
}

/// Decodes a code-block and dequantizes its coefficients into its band.
fn decode_block(
    plane: &mut Plane,
    block: &CodeBlock,
    orientation: u8,
    coding: &BandCoding,
    budget: &mut Budget,
) -> Result<()> {
    if block.passes == 0 {
        return Ok(());
    }
    let planes = coding
        .planes
        .checked_sub(block.zero_planes)
        .filter(|&n| n != 0)
        .ok_or_else(|| syntax("invalid JPEG 2000 zero bit-plane count", block.start))?;
    let parameters = tier1::Parameters {
        width: block.width,
        height: block.height,
        orientation,
        planes,
        passes: block.passes,
        modes: coding.modes,
    };
    let values = tier1::decode(&block.segments, &parameters, budget)?;
    let width = u32_to_usize(plane.width);
    for (y, row) in values.chunks_exact(block.width).enumerate() {
        let start = (block.y + y) * width + block.x;
        let mut nonzero = false;
        for (x, value) in row.iter().enumerate() {
            let coefficient = value.reconstruct(coding.roi, coding.reversible);
            if coefficient == 0.0 {
                continue;
            }
            plane.materialize(budget)?;
            // Partition::new places every code-block inside its band.
            let slot = plane.values.get_mut(start + x);
            debug_assert!(slot.is_some(), "code-blocks lie inside their band");
            if let Some(slot) = slot {
                *slot = coefficient * coding.step;
                nonzero = true;
            }
        }
        if nonzero {
            plane.include_rows(block.y + y..block.y + y + 1);
        }
    }
    let count = values.len();
    drop(values);
    budget.release::<tier1::Sample>(count);
    Ok(())
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

/// Applies the inverse component transform (T.800 G.2, G.3) to the first
/// three planes. Returns whether all three equal the first, which then
/// stands for them.
fn component_transform(
    planes: &mut [Plane],
    reversible: bool,
    budget: &mut Budget,
) -> Result<bool> {
    // Configuration::validate requires three components for a transform,
    // and Size::validate_config gives them one sampling, hence one grid.
    debug_assert!(
        planes.len() >= 3,
        "a component transform needs three planes"
    );
    let [a, b, c, ..] = planes else {
        return Ok(false);
    };
    debug_assert!(
        [b.width, c.width] == [a.width, a.width]
            && [b.height, c.height] == [a.height, a.height]
            && [b.x0, c.x0] == [a.x0, a.x0]
            && [b.y0, c.y0] == [a.y0, a.y0],
        "transform components share one grid"
    );
    // T.800 G.2/G.3: zero chroma makes all three transformed channels equal to Y.
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

fn allocate_image(size: &Size, budget: &mut Budget) -> Result<Decoded> {
    let mut components = budget.output_with_capacity(size.components.len())?;
    for c in &size.components {
        let width = size.x1.div_ceil(c.dx) - size.x0.div_ceil(c.dx);
        let height = size.y1.div_ceil(c.dy) - size.y0.div_ceil(c.dy);
        components.push(JpxComponent {
            bits: c.bits,
            signed: c.signed,
            width,
            height,
            samples: budget.output(area(width, height)?, 0)?,
        });
    }
    let mut geometry = budget.output(
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
        channel_definitions: Vec::new(),
        image: JpxImage {
            width: size.x1 - size.x0,
            height: size.y1 - size.y0,
            components,
            color: JpxColor::Unspecified,
            alpha: None,
        },
    })
}

struct MainHeader<'a> {
    /// Where the SIZ marker segment, which sizes the image, starts.
    siz: Position,
    size: Size,
    config: Configuration,
    /// The first marker after the main header.
    next: Found,
    ppm: Ppm<'a>,
}

/// The state of a codestream that its tile-parts share.
struct Stream<'a, 'd> {
    ppm: Ppm<'a>,
    diagnostics: &'d mut Diagnostics,
    /// Whether an inconsistent tile-part count was reported.
    part_count_reported: bool,
}

/// Decodes the codestream that `reader` holds, from its SOC marker to its
/// EOC marker. Positions are those of `reader`.
pub(super) fn decode(
    mut reader: Cursor<'_>,
    pixels: usize,
    budget: &mut Budget,
    diagnostics: &mut Diagnostics,
) -> Result<Decoded> {
    let MainHeader {
        siz,
        size,
        config,
        mut next,
        ppm,
    } = read_header(&mut reader, pixels, budget)?;
    let mut stream = Stream {
        ppm,
        diagnostics,
        part_count_reported: false,
    };
    let count = area(size.columns, size.rows)?;
    let mut tiles = budget.with_capacity(count).at(siz)?;
    tiles.resize_with(count, || None::<Tile>);
    let mut image = allocate_image(&size, budget).at(siz)?;
    loop {
        let sot = next;
        let length = tile_part(
            &mut reader,
            sot,
            &size,
            &config,
            &mut tiles,
            &mut stream,
            budget,
        )
        .at(sot.position)?;
        next = marker(&mut reader, budget)?;
        if next.marker == Marker::EOC {
            break;
        }
        if length == 0 || next.marker != Marker::SOT {
            return Err(syntax("missing JPEG 2000 SOT or EOC", next.position));
        }
    }
    finish(&reader)?;
    stream.ppm.finish()?;
    for tile in tiles {
        tile.ok_or_else(|| syntax("missing JPEG 2000 tile", next.position))?
            .reconstruct(&size, &mut image.image, budget)?;
    }
    Ok(image)
}

/// Reads the tile-part that starts with the SOT marker `sot` and decodes its
/// packets. Returns its declared length, zero for one that runs to EOC.
fn tile_part<'a>(
    reader: &mut Cursor<'a>,
    sot: Found,
    size: &Size,
    config: &Configuration,
    tiles: &mut [Option<Tile>],
    stream: &mut Stream<'a, '_>,
    budget: &mut Budget,
) -> Result<usize> {
    let (mut part, slot) = read_part(reader, sot, tiles)?;
    let mut local = if slot.is_none() {
        Some(config.copy(budget)?)
    } else {
        None
    };
    let ppt = tile_header(&mut part.reader, &mut local, slot, budget)?;
    if let Some(local) = local {
        *slot = Some(Tile::new(
            size,
            u32::from(part.index),
            local,
            sot.position,
            budget,
        )?);
    }
    let tile = slot
        .as_mut()
        .ok_or_else(|| syntax("missing JPEG 2000 tile", sot.position))?;
    tile.start_part(&part, sot.position, stream)?;
    if stream.ppm.fragments.present() && ppt.present() {
        return Err(syntax("simultaneous JPEG 2000 PPM and PPT", sot.position));
    }
    let mut packed = if stream.ppm.fragments.present() {
        Some(stream.ppm.next(budget)?)
    } else {
        ppt.headers(budget)?
    };
    let uses_packed = packed.is_some();
    if !part.reader.at_end() || packed.as_ref().is_some_and(|p| p.remaining() != 0) {
        if tile
            .packed_headers
            .is_some_and(|previous| previous != uses_packed)
        {
            if tile.packed_headers == Some(true) {
                return Err(unsupported(
                    "JPEG 2000 PPT headers preceding a later tile-part are not implemented",
                    sot.position,
                ));
            }
            return Err(syntax(
                "mixed inline and packed JPEG 2000 tile headers",
                sot.position,
            ));
        }
        tile.packed_headers = Some(uses_packed);
    }
    tile.read_packets(&mut part.reader, packed.as_mut(), ppt.present(), budget)?;
    Ok(part.length)
}

struct Part<'a> {
    /// `Isot`, the tile's index.
    index: u16,
    number: u16,
    count: u16,
    length: usize,
    /// The tile-part header and packets, after the SOT marker segment.
    reader: Cursor<'a>,
}

/// Reads the SOT marker segment of a tile-part and returns the tile-part
/// with its tile's slot in `tiles`.
fn read_part<'a, 't>(
    reader: &mut Cursor<'a>,
    sot: Found,
    tiles: &'t mut [Option<Tile>],
) -> Result<(Part<'a>, &'t mut Option<Tile>)> {
    let mut fields = segment(reader)?;
    let index = fields.u16()?;
    let length = u32_to_usize(fields.u32()?);
    let number = u16::from(fields.u8()?);
    let count = u16::from(fields.u8()?);
    finish(&fields)?;
    let slot = tiles
        .get_mut(usize::from(index))
        .filter(|_| number != 255)
        .ok_or_else(|| syntax("invalid JPEG 2000 tile-part index", sot.position))?;
    let data = reader.data();
    let end = if length == 0 {
        // A final tile-part (Psot 0) runs to the EOC marker, which must come
        // after its own SOT segment (T.800 A.4.2).
        if !data.ends_with(&Marker::EOC.bytes()) {
            return Err(syntax(
                "unterminated JPEG 2000 final tile-part",
                sot.position,
            ));
        }
        let end = data.len() - 2;
        if end < reader.offset() {
            return Err(syntax(
                "JPEG 2000 final tile-part overlaps EOC",
                sot.position,
            ));
        }
        end
    } else {
        sot.offset
            .checked_add(length)
            .filter(|&end| end >= reader.offset() && end <= data.len())
            .ok_or_else(|| syntax("invalid JPEG 2000 tile-part length", sot.position))?
    };
    let part = Part {
        index,
        number,
        count,
        length,
        reader: reader.take_cursor(end - reader.offset())?,
    };
    Ok((part, slot))
}

fn tile_header<'a>(
    reader: &mut Cursor<'a>,
    local: &mut Option<Configuration>,
    tile: &mut Option<Tile>,
    budget: &mut Budget,
) -> Result<Fragments<'a>> {
    let mut ppt = Fragments::default();
    let mut seen_poc = false;
    loop {
        let found = marker(reader, budget)?;
        let (code, position) = (found.marker, found.position);
        if code == Marker::SOD {
            break;
        }
        // T.800 Table A.3: main-header and in-packet markers.
        if matches!(
            code,
            Marker::SOC
                | Marker::SIZ
                | Marker::TLM
                | Marker::PLM
                | Marker::PPM
                | Marker::CRG
                | Marker::SOT
                | Marker::SOP
                | Marker::EPH
                | Marker::EOC
        ) {
            return Err(syntax("misplaced JPEG 2000 tile-header marker", position));
        }
        if code == Marker::POC {
            if seen_poc {
                return Err(syntax("repeated JPEG 2000 POC in one header", position));
            }
            seen_poc = true;
        }
        let mut fields = segment(reader)?;
        if code == Marker::PPT {
            ppt.push(&mut fields, budget)?;
        } else if let Some(local) = local {
            local.marker(code, position, &mut fields, budget)?;
        } else {
            let tile = tile
                .as_mut()
                .ok_or_else(|| syntax("missing JPEG 2000 tile", position))?;
            match code {
                Marker::POC => {
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
                Marker::PLT | Marker::COM => {
                    tile.config.marker(code, position, &mut fields, budget)?;
                }
                Marker::COD | Marker::COC | Marker::QCD | Marker::QCC | Marker::RGN => {
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

fn read_header<'a>(
    reader: &mut Cursor<'a>,
    pixels: usize,
    budget: &mut Budget,
) -> Result<MainHeader<'a>> {
    let soc = marker(reader, budget)?;
    let siz = marker(reader, budget)?;
    if soc.marker != Marker::SOC || siz.marker != Marker::SIZ {
        return Err(syntax("missing JPEG 2000 SOC or SIZ", soc.position));
    }
    let size = read_size(&mut segment(reader)?, pixels, budget)?;
    let mut config = Configuration::new(size.components.len(), budget).at(siz.position)?;
    let mut ppm = Ppm::default();
    let mut next = marker(reader, budget)?;
    while next.marker != Marker::SOT {
        if next.marker == Marker::EOC {
            return Err(syntax("JPEG 2000 codestream has no tiles", next.position));
        }
        if matches!(
            next.marker,
            Marker::SOC
                | Marker::SIZ
                | Marker::PLT
                | Marker::PPT
                | Marker::SOD
                | Marker::SOP
                | Marker::EPH
        ) {
            return Err(syntax(
                "misplaced JPEG 2000 main-header marker",
                next.position,
            ));
        }
        let mut fields = segment(reader)?;
        if next.marker == Marker::PPM {
            if fields.remaining() < 5 {
                return Err(syntax("short JPEG 2000 PPM marker", next.position));
            }
            ppm.fragments.push(&mut fields, budget).at(next.position)?;
        } else {
            config
                .marker(next.marker, next.position, &mut fields, budget)
                .at(next.position)?;
        }
        next = marker(reader, budget)?;
    }
    config.validate(next.position)?;
    Ok(MainHeader {
        siz: siz.position,
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
        // T.800 G.2/G.3 with Y=10 and either Cb=4 or Cr=4, then nearest rounding.
        for (reversible, cb, cr, expected) in [
            (true, 0.0, 4.0, [13, 9, 9]),
            (true, 4.0, 0.0, [9, 9, 13]),
            (false, 0.0, 4.0, [16, 7, 10]),
            (false, 4.0, 0.0, [10, 9, 17]),
        ] {
            let mut budget = Budget::new(&Limits::default(), crate::jpx::MESSAGES);
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
