//! Packet headers, tag trees and contributions, T.800 B.9–B.10.

use pagura_core::bytes::{BitCursor, Cursor, J2kFf7};
use pagura_core::convert::u32_to_usize;
use pagura_core::{Error, Position, Result};

use crate::budget::Budget;

use super::coding::PacketMarkers;
use super::marker::Marker;
use super::packed::Headers;
use super::progression::{Precinct, band_bounds};
use super::segments::{self, Segment};
use super::syntax;
use super::wavelet::Plane;

/// Packet header bits: most significant first, with a stuffed zero bit
/// after each 0xFF byte (T.800 B.10.1).
type Bits<'a> = BitCursor<'a, J2kFf7>;

/// Reads a `count`-bit number (T.800 B.10.7.1).
fn number(bits: &mut Bits<'_>, count: u32) -> Result<usize> {
    if count >= usize::BITS {
        return Err(Error::limit(
            "JPEG 2000 packet number exceeds address space",
        ));
    }
    let mut value = 0_u64;
    let mut left = count;
    while left != 0 {
        let take = left.min(32);
        value = (value << take) | u64::from(bits.bits(take)?);
        left -= take;
    }
    usize::try_from(value)
        .map_err(|_| Error::limit("JPEG 2000 packet number exceeds address space"))
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
    fn new(width: usize, height: usize, budget: &mut Budget) -> Result<Self> {
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
        bits: &mut Bits<'_>,
        x: usize,
        y: usize,
        threshold: usize,
        budget: &mut Budget,
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
    pub(super) x: usize,
    pub(super) y: usize,
    pub(super) width: usize,
    pub(super) height: usize,
    pub(super) zero_planes: u8,
    pub(super) passes: usize,
    pub(super) segments: Vec<Segment>,
    /// Where the block's first contribution starts.
    pub(super) start: Position,
    lblock: u8,
}

pub(super) struct Band {
    pub(super) plane: Plane,
    pub(super) orientation: u8,
    pub(super) partitions: Vec<Partition>,
}

pub(super) struct Partition {
    pub(super) blocks: Vec<CodeBlock>,
    columns: usize,
    inclusion: Tree,
    zero_planes: Tree,
}

impl Band {
    pub(super) fn new(
        plane: Plane,
        orientation: u8,
        block: [u8; 2],
        precincts: &[Precinct],
        budget: &mut Budget,
    ) -> Result<Self> {
        let mut partitions = budget.with_capacity(precincts.len())?;
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
        budget: &mut Budget,
    ) -> Result<Self> {
        let [x0, y0, x1, y1] = bounds;
        let (w, h) = (1_u32 << exponents[0], 1_u32 << exponents[1]);
        let (bx0, by0, bx1, by1) = (x0 / w, y0 / h, x1.div_ceil(w), y1.div_ceil(h));
        let columns = if x0 == x1 { 0 } else { u32_to_usize(bx1 - bx0) };
        let rows = if y0 == y1 { 0 } else { u32_to_usize(by1 - by0) };
        let count = columns
            .checked_mul(rows)
            .ok_or_else(|| Error::limit("JPEG 2000 code-block count overflow"))?;
        let mut blocks = budget.with_capacity(count)?;
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
                        x: u32_to_usize(sx - plane.x0),
                        y: u32_to_usize(sy - plane.y0),
                        width: u32_to_usize(ex - sx),
                        height: u32_to_usize(ey - sy),
                        zero_planes: 0,
                        passes: 0,
                        segments: Vec::new(),
                        start: Position::Unknown,
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
        bits: &mut Bits<'_>,
        layer: usize,
        modes: u8,
        band: usize,
        contributions: &mut Vec<Contribution>,
        budget: &mut Budget,
    ) -> Result<()> {
        for (index, block) in self.blocks.iter_mut().enumerate() {
            budget.work(1)?;
            // There are columns × rows blocks, so with blocks there are columns.
            let (x, y) = (index % self.columns, index / self.columns);
            let included = if block.passes == 0 {
                self.inclusion.below(bits, x, y, layer + 1, budget)?
            } else {
                bits.bit()?
            };
            if !included {
                continue;
            }
            if block.passes == 0 {
                let mut threshold = 1_u8;
                while !self
                    .zero_planes
                    .below(bits, x, y, usize::from(threshold), budget)?
                {
                    threshold += 1;
                    if threshold > 32 {
                        return Err(syntax(
                            "JPEG 2000 zero bit-plane tag tree exceeds precision",
                            bits.position(),
                        ));
                    }
                }
                block.zero_planes = threshold - 1;
            }
            let count = passes(bits)?;
            if block.passes + count > segments::MAX_PASSES {
                return Err(syntax(
                    "JPEG 2000 code-block has too many coding passes",
                    bits.position(),
                ));
            }
            while bits.bit()? {
                budget.work(1)?;
                block.lblock += 1;
                if block.lblock > 31 {
                    return Err(syntax(
                        "JPEG 2000 code-block length exponent overflow",
                        bits.position(),
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
                    budget.push(&mut block.segments, Segment::new(block.passes))?;
                }
                let segment_index = block.segments.len() - 1;
                let segment = &mut block.segments[segment_index];
                let new_passes = remaining.min(segments::end(segment.first, modes) - block.passes);
                // T.800 B.10.7.1: Lblock plus floor(log2) of the passes.
                let length = number(bits, u32::from(block.lblock) + new_passes.ilog2())?;
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

fn passes(bits: &mut Bits<'_>) -> Result<usize> {
    if !bits.bit()? {
        return Ok(1);
    }
    if !bits.bit()? {
        return Ok(2);
    }
    let count = number(bits, 2)?;
    if count < 3 {
        return Ok(3 + count);
    }
    let count = number(bits, 5)?;
    if count < 31 {
        return Ok(6 + count);
    }
    Ok(37 + number(bits, 7)?)
}

pub(super) struct Parameters {
    pub(super) layer: usize,
    pub(super) precinct: usize,
    pub(super) markers: PacketMarkers,
    pub(super) sequence: u16,
    pub(super) modes: u8,
    pub(super) tile_headers: bool,
}

fn headers(
    reader: &mut Cursor<'_>,
    bands: &mut [Band],
    parameters: &Parameters,
    budget: &mut Budget,
) -> Result<Vec<Contribution>> {
    let mut bits = Bits::new(reader.clone());
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
    // T.800 B.10.1: a header that ends on 0xFF is followed by a stuffed byte.
    bits.align()?;
    let rest = bits.byte_cursor()?;
    if parameters.markers.eph && rest.take_array()? != Marker::EPH.bytes() {
        return Err(rest.error("missing JPEG 2000 EPH marker"));
    }
    *reader = rest.clone();
    Ok(contributions)
}

pub(super) fn read(
    body: &mut Cursor<'_>,
    packed: Option<&mut Headers<'_>>,
    bands: &mut [Band],
    parameters: &Parameters,
    budget: &mut Budget,
) -> Result<()> {
    budget.work(1)?;
    // T.800 A.8.1: Lsop is 4, then the packet sequence number.
    if body.rest().starts_with(&Marker::SOP.bytes())
        && (!parameters.markers.sop
            || body.take_array()? != Marker::SOP.bytes()
            || body.u16()? != 4
            || body.u16()? != parameters.sequence)
    {
        return Err(body.error("invalid JPEG 2000 SOP marker"));
    }
    let contributions = match packed {
        Some(packed) => packed.header(|reader| headers(reader, bands, parameters, budget))?,
        None => headers(body, bands, parameters, budget)?,
    };
    for contribution in contributions {
        // headers() made each contribution from these bands, blocks and
        // segments, so the indices are in range.
        let block = &mut bands[contribution.band].partitions[parameters.precinct].blocks
            [contribution.block];
        let source = body.position();
        if contribution.segment == 0 && block.segments.first().is_some_and(|s| s.data.is_empty()) {
            block.start = source;
        }
        if parameters.tile_headers && contribution.length > body.remaining() {
            return Err(super::unsupported(
                "JPEG 2000 PPT headers preceding a later tile-part are not implemented",
                source,
            ));
        }
        let bytes = body.take(contribution.length)?;
        if bytes.last() == Some(&255) {
            return Err(syntax(
                "JPEG 2000 code-block contribution ends in FF",
                Position::Buffer(bytes.len() - 1).within(source),
            ));
        }
        block.segments[contribution.segment].append(bytes, source, budget)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Bits, number};
    use pagura_core::ErrorKind;
    use pagura_core::bytes::Cursor;

    #[test]
    fn numbers_wider_than_32_bits_are_read_in_parts() {
        // 0xFF is followed by a stuffed byte whose high bit is not data.
        let data = [0x12, 0xff, 0x2b, 0x56, 0x78, 0x9a, 0xbc];
        let mut bits = Bits::new(Cursor::new(&data));
        assert_eq!(number(&mut bits, 4).unwrap(), 0x1);
        if usize::BITS == 64 {
            let wide = number(&mut bits, 35).unwrap();
            assert_eq!(u64::try_from(wide).unwrap(), 0x1_7fab_5678);
        }
        let error = number(&mut bits, usize::BITS).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit);
    }
}
