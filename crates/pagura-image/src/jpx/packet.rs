//! Packet headers, tag trees and code-block contributions (Annex B).

use pagura_core::{Error, Result};

use super::io::{Budget, Reader, syntax};
use super::wavelet::Plane;

struct Bits<'a, 'b> {
    reader: &'a mut Reader<'b>,
    byte: u8,
    remaining: u8,
}

impl<'a, 'b> Bits<'a, 'b> {
    fn new(reader: &'a mut Reader<'b>) -> Self {
        Self {
            reader,
            byte: 0,
            remaining: 0,
        }
    }

    fn bit(&mut self) -> Result<bool> {
        if self.remaining == 0 {
            let stuffed = self.byte == 255;
            self.byte = self.reader.u8()?;
            if stuffed && self.byte & 128 != 0 {
                return Err(syntax(
                    "invalid JPEG 2000 packet bit stuffing",
                    self.reader.position() - 1,
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
        if u16::from(self.byte) & ((1_u16 << self.remaining) - 1) != 0 {
            return Err(syntax(
                "nonzero JPEG 2000 packet padding",
                self.reader.position() - 1,
            ));
        }
        if self.byte == 255 && self.reader.u8()? != 0 {
            return Err(syntax(
                "invalid JPEG 2000 packet padding byte",
                self.reader.position() - 1,
            ));
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
    widths: [usize; 32],
    offsets: [usize; 32],
    levels: usize,
}

impl Tree {
    fn new(width: usize, height: usize, budget: &mut Budget<'_>) -> Result<Self> {
        let mut widths = [0; 32];
        let mut offsets = [0; 32];
        let mut levels = 0;
        let mut count = 0;
        let (mut w, mut h) = (width, height);
        while w != 0 && h != 0 {
            budget.work(1)?;
            if levels == widths.len() {
                return Err(Error::limit("JPEG 2000 tag-tree depth exceeded"));
            }
            widths[levels] = w;
            offsets[levels] = count;
            levels += 1;
            count += w * h;
            if w == 1 && h == 1 {
                break;
            }
            w = w.div_ceil(2);
            h = h.div_ceil(2);
        }
        Ok(Self {
            nodes: budget.filled(count, Node::default())?,
            widths,
            offsets,
            levels,
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
        let mut low = 0;
        for level in (0..self.levels).rev() {
            let index = self.offsets[level] + (y >> level) * self.widths[level] + (x >> level);
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
        Ok(self.nodes[y * self.widths[0] + x]
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
    pub data: Vec<u8>,
    pub base: usize,
    lblock: u8,
}

pub(super) struct Band {
    pub plane: Plane,
    pub orientation: u8,
    pub blocks: Vec<CodeBlock>,
    columns: usize,
    inclusion: Tree,
    zero_planes: Tree,
}

impl Band {
    pub fn new(
        plane: Plane,
        orientation: u8,
        block_exponents: [u8; 2],
        budget: &mut Budget<'_>,
    ) -> Result<Self> {
        let block_width = 1_u32 << block_exponents[0];
        let block_height = 1_u32 << block_exponents[1];
        let x0 = plane.x0 / block_width;
        let y0 = plane.y0 / block_height;
        let x1 = (plane.x0 + plane.width).div_ceil(block_width);
        let y1 = (plane.y0 + plane.height).div_ceil(block_height);
        let columns = if plane.width == 0 {
            0
        } else {
            super::io::usize_from(x1 - x0)?
        };
        let rows = if plane.height == 0 {
            0
        } else {
            super::io::usize_from(y1 - y0)?
        };
        let count = columns
            .checked_mul(rows)
            .ok_or_else(|| Error::limit("JPEG 2000 code-block count overflow"))?;
        budget.work(count)?;
        budget.charge::<CodeBlock>(count)?;
        let mut blocks = Vec::with_capacity(count);
        for y in y0..y0
            + u32::try_from(rows).map_err(|_| Error::limit("JPEG 2000 code-block rows overflow"))?
        {
            for x in x0..x0
                + u32::try_from(columns)
                    .map_err(|_| Error::limit("JPEG 2000 code-block columns overflow"))?
            {
                let start_x = (x * block_width).max(plane.x0);
                let start_y = (y * block_height).max(plane.y0);
                let end_x = ((x + 1) * block_width).min(plane.x0 + plane.width);
                let end_y = ((y + 1) * block_height).min(plane.y0 + plane.height);
                blocks.push(CodeBlock {
                    x: super::io::usize_from(start_x - plane.x0)?,
                    y: super::io::usize_from(start_y - plane.y0)?,
                    width: super::io::usize_from(end_x - start_x)?,
                    height: super::io::usize_from(end_y - start_y)?,
                    zero_planes: 0,
                    passes: 0,
                    data: Vec::new(),
                    base: 0,
                    lblock: 3,
                });
            }
        }
        Ok(Self {
            plane,
            orientation,
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
        contributions: &mut Vec<(usize, usize)>,
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
                contributions.push((0, 0));
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
            let passes = passes(bits)?;
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
            let exponent = u8::try_from(usize::BITS - 1 - passes.leading_zeros())
                .map_err(|_| syntax("JPEG 2000 pass count overflow", bits.reader.position()))?;
            let length_bits = block
                .lblock
                .checked_add(exponent)
                .filter(|&n| u32::from(n) < usize::BITS)
                .ok_or_else(|| Error::limit("JPEG 2000 code-block length exceeds address space"))?;
            let length = bits.number(length_bits)?;
            if block.passes + passes > 88 {
                return Err(syntax(
                    "JPEG 2000 code-block has too many coding passes",
                    bits.reader.position(),
                ));
            }
            contributions.push((passes, length));
        }
        Ok(())
    }
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

pub(super) fn read(
    reader: &mut Reader<'_>,
    bands: &mut [Band],
    layer: usize,
    markers: [bool; 2],
    sequence: u16,
    budget: &mut Budget<'_>,
) -> Result<()> {
    budget.work(1)?;
    if markers[0] && (reader.u16()? != 0xff91 || reader.u16()? != 4 || reader.u16()? != sequence) {
        return Err(syntax("invalid JPEG 2000 SOP marker", reader.position()));
    }
    let mut bits = Bits::new(reader);
    if !bits.bit()? {
        bits.finish()?;
        if markers[1] && reader.u16()? != 0xff92 {
            return Err(syntax("missing JPEG 2000 EPH marker", reader.position()));
        }
        return Ok(());
    }
    let count = bands.iter().map(|b| b.blocks.len()).sum();
    budget.charge::<(usize, usize)>(count)?;
    let mut contributions = Vec::with_capacity(count);
    for band in &mut *bands {
        band.header(&mut bits, layer, &mut contributions, budget)?;
    }
    bits.finish()?;
    if markers[1] && reader.u16()? != 0xff92 {
        return Err(syntax("missing JPEG 2000 EPH marker", reader.position()));
    }
    for (block, (passes, length)) in bands
        .iter_mut()
        .flat_map(|b| &mut b.blocks)
        .zip(contributions)
    {
        if passes == 0 {
            continue;
        }
        if block.data.is_empty() {
            block.base = reader.position();
        }
        let bytes = reader.take(length)?;
        let needed = block
            .data
            .len()
            .checked_add(length)
            .ok_or_else(|| Error::limit("JPEG 2000 code-block length overflow"))?;
        if needed > block.data.capacity() {
            let capacity = needed
                .checked_next_power_of_two()
                .ok_or_else(|| Error::limit("JPEG 2000 code-block capacity overflow"))?;
            // Geometric growth keeps many quality-layer contributions linear.
            // Charge the whole new allocation, including any old bytes it copies.
            budget.charge::<u8>(capacity)?;
            block
                .data
                .try_reserve_exact(capacity - block.data.len())
                .map_err(|_| Error::limit("JPEG 2000 code-block allocation failed"))?;
        }
        budget.work(length)?;
        block.data.extend_from_slice(bytes);
        block.passes += passes;
    }
    Ok(())
}
