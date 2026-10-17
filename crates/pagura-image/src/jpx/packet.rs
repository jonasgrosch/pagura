//! Packet headers, tag trees and contributions, T.800 B.9–B.10.

use pagura_core::{Error, Result};

use super::io::{Budget, Reader, syntax};
use super::progression::{Precinct, band_bounds};
use super::segments::{self, Segment};
use super::wavelet::Plane;

struct Bits<'a, 'b> {
    reader: &'a mut Reader<'b>,
    byte: u8,
    remaining: u8,
    byte_position: usize,
}

impl<'a, 'b> Bits<'a, 'b> {
    fn new(reader: &'a mut Reader<'b>) -> Self {
        let byte_position = reader.position();
        Self {
            reader,
            byte: 0,
            remaining: 0,
            byte_position,
        }
    }

    fn bit(&mut self) -> Result<bool> {
        if self.remaining == 0 {
            let stuffed = self.byte == 255;
            self.byte_position = self.reader.position();
            self.byte = self.reader.u8()?;
            if stuffed && self.byte & 128 != 0 {
                return Err(syntax(
                    "invalid JPEG 2000 packet bit stuffing",
                    self.byte_position,
                ));
            }
            self.remaining = if stuffed { 7 } else { 8 };
        }
        self.remaining -= 1;
        Ok((self.byte >> self.remaining) & 1 != 0)
    }

    fn number(&mut self, count: u8) -> Result<usize> {
        if u32::from(count) >= usize::BITS {
            return Err(Error::limit(
                "JPEG 2000 packet number exceeds address space",
            ));
        }
        let mut value = 0_usize;
        for _ in 0..count {
            value = (value << 1) | usize::from(self.bit()?);
        }
        Ok(value)
    }

    fn finish(self) -> Result<()> {
        if self.byte == 255 {
            let position = self.reader.position();
            if self.reader.u8()? & 128 != 0 {
                return Err(syntax("invalid JPEG 2000 packet padding byte", position));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Default)]
struct Node {
    low: usize,
    value: Option<usize>,
}

struct Tree {
    nodes: Vec<Node>,
    width: usize,
    height: usize,
}

impl Tree {
    fn new(width: usize, height: usize, budget: &mut Budget<'_>) -> Result<Self> {
        let mut count = 0;
        let (mut w, mut h) = (width, height);
        while w != 0 && h != 0 {
            budget.work(1)?;
            count += w * h;
            if w == 1 && h == 1 {
                break;
            }
            w = w.div_ceil(2);
            h = h.div_ceil(2);
        }
        Ok(Self {
            nodes: budget.filled(count, Node::default())?,
            width,
            height,
        })
    }

    fn below(
        &mut self,
        bits: &mut Bits<'_, '_>,
        x: usize,
        y: usize,
        threshold: usize,
        budget: &mut Budget<'_>,
    ) -> Result<bool> {
        let mut path = [0; 32];
        let (mut w, mut h, mut offset, mut levels) = (self.width, self.height, 0, 0);
        loop {
            budget.work(1)?;
            if levels == path.len() {
                return Err(Error::limit("JPEG 2000 tag-tree depth exceeded"));
            }
            path[levels] = offset + (y >> levels) * w + (x >> levels);
            levels += 1;
            if w == 1 && h == 1 {
                break;
            }
            offset += w * h;
            w = w.div_ceil(2);
            h = h.div_ceil(2);
        }
        let mut low = 0;
        for &index in path[..levels].iter().rev() {
            let node = &mut self.nodes[index];
            node.low = node.low.max(low);
            while node.low < threshold && node.value.is_none() {
                budget.work(1)?;
                if bits.bit()? {
                    node.value = Some(node.low);
                } else {
                    node.low += 1;
                }
            }
            low = node.value.unwrap_or(node.low);
        }
        Ok(self.nodes[y * self.width + x]
            .value
            .is_some_and(|v| v < threshold))
    }
}

pub(super) struct CodeBlock {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
    pub zero_planes: u8,
    pub passes: usize,
    pub segments: Vec<Segment>,
    pub base: usize,
    lblock: u8,
}

pub(super) struct Band {
    pub plane: Plane,
    pub orientation: u8,
    pub partitions: Vec<Partition>,
}

pub(super) struct Partition {
    pub blocks: Vec<CodeBlock>,
    columns: usize,
    inclusion: Tree,
    zero_planes: Tree,
}

impl Band {
    pub fn new(
        plane: Plane,
        orientation: u8,
        block: [u8; 2],
        precincts: &[Precinct],
        budget: &mut Budget<'_>,
    ) -> Result<Self> {
        budget.charge::<Partition>(precincts.len())?;
        let mut partitions = Vec::with_capacity(precincts.len());
        for precinct in precincts {
            partitions.push(Partition::new(
                &plane,
                band_bounds(precinct.bounds, orientation),
                block,
                budget,
            )?);
        }
        Ok(Self {
            plane,
            orientation,
            partitions,
        })
    }
}

impl Partition {
    fn new(
        plane: &Plane,
        bounds: [u32; 4],
        exponents: [u8; 2],
        budget: &mut Budget<'_>,
    ) -> Result<Self> {
        let [x0, y0, x1, y1] = bounds;
        let (w, h) = (1_u32 << exponents[0], 1_u32 << exponents[1]);
        let (bx0, by0, bx1, by1) = (x0 / w, y0 / h, x1.div_ceil(w), y1.div_ceil(h));
        let columns = if x0 == x1 {
            0
        } else {
            super::io::usize_from(bx1 - bx0)?
        };
        let rows = if y0 == y1 {
            0
        } else {
            super::io::usize_from(by1 - by0)?
        };
        let count = columns
            .checked_mul(rows)
            .ok_or_else(|| Error::limit("JPEG 2000 code-block count overflow"))?;
        budget.charge::<CodeBlock>(count)?;
        let mut blocks = Vec::with_capacity(count);
        if count != 0 {
            for y in by0..by1 {
                for x in bx0..bx1 {
                    let (sx, sy, ex, ey) = (
                        (x * w).max(x0),
                        (y * h).max(y0),
                        ((x + 1) * w).min(x1),
                        ((y + 1) * h).min(y1),
                    );
                    blocks.push(CodeBlock {
                        x: super::io::usize_from(sx - plane.x0)?,
                        y: super::io::usize_from(sy - plane.y0)?,
                        width: super::io::usize_from(ex - sx)?,
                        height: super::io::usize_from(ey - sy)?,
                        zero_planes: 0,
                        passes: 0,
                        segments: Vec::new(),
                        base: 0,
                        lblock: 3,
                    });
                }
            }
        }
        Ok(Self {
            blocks,
            columns,
            inclusion: Tree::new(columns, rows, budget)?,
            zero_planes: Tree::new(columns, rows, budget)?,
        })
    }

    fn header(
        &mut self,
        bits: &mut Bits<'_, '_>,
        layer: usize,
        modes: u8,
        band: usize,
        contributions: &mut Vec<Contribution>,
        budget: &mut Budget<'_>,
    ) -> Result<()> {
        for (index, block) in self.blocks.iter_mut().enumerate() {
            budget.work(1)?;
            let included = if block.passes == 0 {
                self.inclusion.below(
                    bits,
                    index % self.columns,
                    index / self.columns,
                    layer + 1,
                    budget,
                )?
            } else {
                bits.bit()?
            };
            if !included {
                continue;
            }
            if block.passes == 0 {
                let mut threshold = 1;
                while !self.zero_planes.below(
                    bits,
                    index % self.columns,
                    index / self.columns,
                    threshold,
                    budget,
                )? {
                    threshold += 1;
                    if threshold > 32 {
                        return Err(syntax(
                            "JPEG 2000 zero bit-plane tag tree exceeds precision",
                            bits.reader.position(),
                        ));
                    }
                }
                block.zero_planes = u8::try_from(threshold - 1).map_err(|_| {
                    syntax("JPEG 2000 zero bit-plane overflow", bits.reader.position())
                })?;
            }
            let count = passes(bits)?;
            if block.passes + count > 88 {
                return Err(syntax(
                    "JPEG 2000 code-block has too many coding passes",
                    bits.reader.position(),
                ));
            }
            while bits.bit()? {
                budget.work(1)?;
                block.lblock += 1;
                if block.lblock > 31 {
                    return Err(syntax(
                        "JPEG 2000 code-block length exponent overflow",
                        bits.reader.position(),
                    ));
                }
            }
            let mut remaining = count;
            while remaining != 0 {
                let need_new = block
                    .segments
                    .last()
                    .is_none_or(|s| s.first + s.passes == segments::end(s.first, modes));
                if need_new {
                    budget.push(
                        &mut block.segments,
                        Segment {
                            first: block.passes,
                            passes: 0,
                            data: Vec::new(),
                            base: 0,
                            spans: Vec::new(),
                        },
                    )?;
                }
                let segment_index = block.segments.len() - 1;
                let segment = &mut block.segments[segment_index];
                let new_passes = remaining.min(segments::end(segment.first, modes) - block.passes);
                let exponent = u8::try_from(new_passes.ilog2())
                    .map_err(|_| syntax("JPEG 2000 pass count overflow", bits.reader.position()))?;
                let length_bits = block
                    .lblock
                    .checked_add(exponent)
                    .filter(|&n| u32::from(n) < usize::BITS)
                    .ok_or_else(|| {
                        Error::limit("JPEG 2000 code-block length exceeds address space")
                    })?;
                let length = bits.number(length_bits)?;
                budget.push(
                    contributions,
                    Contribution {
                        band,
                        block: index,
                        segment: segment_index,
                        length,
                    },
                )?;
                segment.passes += new_passes;
                block.passes += new_passes;
                remaining -= new_passes;
            }
        }
        Ok(())
    }
}

struct Contribution {
    band: usize,
    block: usize,
    segment: usize,
    length: usize,
}

fn passes(bits: &mut Bits<'_, '_>) -> Result<usize> {
    if !bits.bit()? {
        return Ok(1);
    }
    if !bits.bit()? {
        return Ok(2);
    }
    let count = bits.number(2)?;
    if count < 3 {
        return Ok(3 + count);
    }
    let count = bits.number(5)?;
    if count < 31 {
        return Ok(6 + count);
    }
    Ok(37 + bits.number(7)?)
}

pub(super) struct Parameters {
    pub layer: usize,
    pub precinct: usize,
    pub markers: [bool; 2],
    pub sequence: u16,
    pub modes: u8,
    pub tile_headers: bool,
}

fn headers(
    reader: &mut Reader<'_>,
    bands: &mut [Band],
    parameters: &Parameters,
    budget: &mut Budget<'_>,
) -> Result<Vec<Contribution>> {
    let start = reader.offset;
    let mut bits = Bits::new(reader);
    let mut contributions = Vec::new();
    if bits.bit()? {
        for (index, band) in bands.iter_mut().enumerate() {
            band.partitions[parameters.precinct].header(
                &mut bits,
                parameters.layer,
                parameters.modes,
                index,
                &mut contributions,
                budget,
            )?;
        }
    }
    bits.finish()?;
    if parameters.markers[1] && reader.u16()? != 0xff92 {
        return Err(syntax("missing JPEG 2000 EPH marker", reader.position()));
    }
    reader.packet_boundary(start)?;
    Ok(contributions)
}

pub(super) fn read(
    body: &mut Reader<'_>,
    packed: Option<&mut Reader<'_>>,
    bands: &mut [Band],
    parameters: &Parameters,
    budget: &mut Budget<'_>,
) -> Result<()> {
    budget.work(1)?;
    if body.data.get(body.offset..body.offset.saturating_add(2)) == Some(&[255, 145])
        && (!parameters.markers[0]
            || body.u16()? != 0xff91
            || body.u16()? != 4
            || body.u16()? != parameters.sequence)
    {
        return Err(syntax("invalid JPEG 2000 SOP marker", body.position()));
    }
    let contributions = match packed {
        Some(reader) => headers(reader, bands, parameters, budget)?,
        None => headers(body, bands, parameters, budget)?,
    };
    for contribution in contributions {
        let block = &mut bands[contribution.band].partitions[parameters.precinct].blocks
            [contribution.block];
        if contribution.segment == 0 && block.segments[0].data.is_empty() {
            block.base = body.position();
        }
        let segment = &mut block.segments[contribution.segment];
        if segment.data.is_empty() {
            segment.base = body.position();
        }
        let source = body.position();
        if parameters.tile_headers && contribution.length > body.remaining() {
            return Err(super::unsupported(
                "JPEG 2000 PPT headers preceding a later tile-part are not implemented",
                body.position(),
            ));
        }
        let bytes = body.take(contribution.length)?;
        if bytes.last() == Some(&255) {
            return Err(syntax(
                "JPEG 2000 code-block contribution ends in FF",
                source + bytes.len() - 1,
            ));
        }
        if !bytes.is_empty() {
            budget.push(
                &mut segment.spans,
                super::io::Span {
                    logical: segment.data.len(),
                    source,
                },
            )?;
        }
        budget.append(&mut segment.data, bytes)?;
    }
    Ok(())
}
