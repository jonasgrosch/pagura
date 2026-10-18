//! Coefficient bit modelling, T.800 D.1–D.7, Tables D.1–D.7.

use pagura_core::Result;

use super::io::{Budget, syntax};
use super::mq::{Context, Decoder};
use super::segments::{self, Segment};

#[derive(Clone, Copy, Default)]
pub(super) struct Sample {
    magnitude: u32,
    negative: bool,
    visited: bool,
    refined: bool,
    missing: u8,
}

impl Sample {
    /// D.2.1, H.1, E-6–E-8: r=1/2, with a coefficient-specific decoded prefix.
    pub fn reconstruct(self, roi: u8, reversible: bool) -> f64 {
        let mut magnitude = self.magnitude;
        let mut missing = self.missing;
        if roi != 0 && magnitude >= (1_u32 << roi) {
            magnitude >>= roi;
            missing = missing.saturating_sub(roi);
        }
        if magnitude == 0 {
            return 0.0;
        }
        let offset = if reversible && missing == 0 {
            0.0
        } else {
            f64::from(1_u32 << missing) * 0.5
        };
        (f64::from(magnitude) + offset) * if self.negative { -1.0 } else { 1.0 }
    }
}

struct Block {
    samples: Vec<Sample>,
    width: usize,
    height: usize,
    orientation: u8,
    causal: bool,
}

impl Block {
    fn neighbours(&self, x: usize, y: usize) -> (usize, usize, usize, i8, i8) {
        let mut horizontal = 0;
        let mut vertical = 0;
        let mut diagonal = 0;
        let mut sign_h = 0;
        let mut sign_v = 0;
        for dy in -1_i32..=1 {
            for dx in -1_i32..=1 {
                if (dx == 0 && dy == 0) || (self.causal && y % 4 == 3 && dy == 1) {
                    continue;
                }
                let Some(nx) = x
                    .checked_add_signed(dx as isize)
                    .filter(|&n| n < self.width)
                else {
                    continue;
                };
                let Some(ny) = y
                    .checked_add_signed(dy as isize)
                    .filter(|&n| n < self.height)
                else {
                    continue;
                };
                let sample = self.samples[ny * self.width + nx];
                if sample.magnitude == 0 {
                    continue;
                }
                let sign = if sample.negative { -1 } else { 1 };
                if dy == 0 {
                    horizontal += 1;
                    sign_h += sign;
                } else if dx == 0 {
                    vertical += 1;
                    sign_v += sign;
                } else {
                    diagonal += 1;
                }
            }
        }
        (
            horizontal,
            vertical,
            diagonal,
            sign_h.clamp(-1, 1),
            sign_v.clamp(-1, 1),
        )
    }

    fn significance_context(&self, x: usize, y: usize) -> usize {
        let (mut h, mut v, d, _, _) = self.neighbours(x, y);
        if self.orientation == 1 {
            std::mem::swap(&mut h, &mut v);
        }
        if self.orientation == 3 {
            let hv = h + v;
            match (d, hv) {
                (0, 0) => 0,
                (0, 1) => 1,
                (0, _) => 2,
                (1, 0) => 3,
                (1, 1) => 4,
                (1, _) => 5,
                (2, 0) => 6,
                (2, _) => 7,
                _ => 8,
            }
        } else {
            match (h, v, d) {
                (0, 0, 0) => 0,
                (0, 0, 1) => 1,
                (0, 0, _) => 2,
                (0, 1, _) => 3,
                (0, _, _) => 4,
                (1, 0, 0) => 5,
                (1, 0, _) => 6,
                (1, _, _) => 7,
                _ => 8,
            }
        }
    }

    fn sign(&self, decoder: &mut Entropy<'_>, x: usize, y: usize) -> Result<bool> {
        let (_, _, _, mut h, mut v) = self.neighbours(x, y);
        let prediction = h < 0 || (h == 0 && v < 0);
        if prediction {
            h = -h;
            v = -v;
        }
        let context = match (h, v) {
            (0, 0) => 9,
            (0, _) => 10,
            (_, -1) => 11,
            (_, 0) => 12,
            _ => 13,
        };
        if decoder.raw() {
            decoder.bit(0)
        } else {
            Ok(decoder.bit(context)? ^ prediction)
        }
    }

    fn make_significant(
        &mut self,
        decoder: &mut Entropy<'_>,
        x: usize,
        y: usize,
        plane: u8,
    ) -> Result<()> {
        let negative = self.sign(decoder, x, y)?;
        let sample = &mut self.samples[y * self.width + x];
        sample.magnitude = 1 << plane;
        sample.negative = negative;
        sample.missing = plane;
        Ok(())
    }

    fn propagation(&mut self, decoder: &mut Entropy<'_>, plane: u8) -> Result<()> {
        for stripe in (0..self.height).step_by(4) {
            for x in 0..self.width {
                for y in stripe..(stripe + 4).min(self.height) {
                    let index = y * self.width + x;
                    if self.samples[index].magnitude != 0 || self.significance_context(x, y) == 0 {
                        continue;
                    }
                    self.samples[index].visited = true;
                    if decoder.bit(self.significance_context(x, y))? {
                        self.make_significant(decoder, x, y, plane)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn refinement(&mut self, decoder: &mut Entropy<'_>, plane: u8) -> Result<()> {
        for stripe in (0..self.height).step_by(4) {
            for x in 0..self.width {
                for y in stripe..(stripe + 4).min(self.height) {
                    let index = y * self.width + x;
                    let sample = self.samples[index];
                    if sample.magnitude == 0 || sample.visited {
                        continue;
                    }
                    let context = if sample.refined {
                        16
                    } else {
                        14 + usize::from(self.significance_context(x, y) != 0)
                    };
                    let value = decoder.bit(context)?;
                    self.samples[index].magnitude |= u32::from(value) << plane;
                    self.samples[index].refined = true;
                    self.samples[index].missing = plane;
                }
            }
        }
        Ok(())
    }

    fn cleanup(&mut self, decoder: &mut Entropy<'_>, plane: u8) -> Result<()> {
        for stripe in (0..self.height).step_by(4) {
            for x in 0..self.width {
                let end = (stripe + 4).min(self.height);
                let run_mode = end - stripe == 4
                    && (stripe..end).all(|y| {
                        let s = self.samples[y * self.width + x];
                        s.magnitude == 0 && !s.visited && self.significance_context(x, y) == 0
                    });
                let mut start = stripe;
                if run_mode {
                    if !decoder.bit(17)? {
                        continue;
                    }
                    let run = 2 * usize::from(decoder.bit(18)?) + usize::from(decoder.bit(18)?);
                    start += run;
                    self.make_significant(decoder, x, start, plane)?;
                    start += 1;
                }
                for y in start..end {
                    let sample = self.samples[y * self.width + x];
                    if sample.magnitude == 0
                        && !sample.visited
                        && decoder.bit(self.significance_context(x, y))?
                    {
                        self.make_significant(decoder, x, y, plane)?;
                    }
                }
            }
        }
        for sample in &mut self.samples {
            sample.visited = false;
        }
        Ok(())
    }
}

pub(super) struct Parameters {
    pub width: usize,
    pub height: usize,
    pub orientation: u8,
    pub planes: u8,
    pub passes: usize,
    pub modes: u8,
}

enum Entropy<'a> {
    Mq(Decoder<'a>),
    Raw(Raw<'a>),
}

impl Entropy<'_> {
    fn bit(&mut self, context: usize) -> Result<bool> {
        match self {
            Self::Mq(decoder) => decoder.bit(context),
            Self::Raw(decoder) => decoder.bit(),
        }
    }
    fn raw(&self) -> bool {
        matches!(self, Self::Raw(_))
    }
}

struct Raw<'a> {
    data: &'a [u8],
    offset: usize,
    base: usize,
    byte: u8,
    remaining: u8,
    synthetic: bool,
}

impl Raw<'_> {
    fn finish(mut self, predictable: bool) -> Result<()> {
        for (offset, bytes) in self.data.windows(2).enumerate() {
            if bytes[0] == 255 && bytes[1] & 128 != 0 {
                return Err(syntax("invalid JPEG 2000 bypass bit stuffing", offset + 1));
            }
        }
        if !predictable {
            return Ok(());
        }
        // D.6: unused raw bits alternate 0,1; a final FF requires a stuffed byte.
        if self.remaining == 0 && self.byte == 255 {
            self.byte =
                self.data.get(self.offset).copied().ok_or_else(|| {
                    syntax("missing JPEG 2000 bypass termination byte", self.offset)
                })?;
            self.offset += 1;
            self.remaining = 7;
            if self.byte & 128 != 0 {
                return Err(syntax(
                    "invalid JPEG 2000 bypass bit stuffing",
                    self.offset - 1,
                ));
            }
        }
        let mask = (1_u16 << self.remaining) - 1;
        let expected = (0..self.remaining).fold(0_u16, |v, i| (v << 1) | u16::from(i % 2));
        if self.synthetic
            || u16::from(self.byte) & mask != expected
            || self.offset != self.data.len()
        {
            return Err(syntax(
                "invalid JPEG 2000 predictable bypass termination",
                self.offset.saturating_sub(1),
            ));
        }
        Ok(())
    }

    fn bit(&mut self) -> Result<bool> {
        if self.remaining == 0 {
            let stuffed = self.byte == 255;
            let next = self.data.get(self.offset).copied();
            self.synthetic |= next.is_none();
            self.byte = next.unwrap_or(255);
            if stuffed && next.is_some() && self.byte & 128 != 0 {
                return Err(syntax(
                    "invalid JPEG 2000 bypass bit stuffing",
                    self.base + self.offset,
                ));
            }
            self.offset += usize::from(next.is_some());
            self.remaining = if stuffed { 7 } else { 8 };
        }
        self.remaining -= 1;
        Ok(self.byte & (1 << self.remaining) != 0)
    }
}

pub(super) fn decode(
    segments: &[Segment],
    p: &Parameters,
    budget: &mut Budget<'_>,
) -> Result<Vec<Sample>> {
    let base = segments.first().map_or(0, |s| s.base);
    let valid_modes = segments::BYPASS
        | segments::RESET
        | segments::TERMINATE
        | segments::CAUSAL
        | segments::PREDICTABLE
        | segments::SYMBOLS;
    if p.planes == 0
        || p.planes > 30
        || p.passes == 0
        || p.passes > usize::from(p.planes) * 3 - 2
        || p.modes & !valid_modes != 0
    {
        return Err(syntax("invalid JPEG 2000 bit planes", base));
    }
    let count = p.width * p.height;
    budget.work(
        count
            .checked_mul(p.passes)
            .ok_or_else(|| pagura_core::Error::limit("JPEG 2000 code-block work overflow"))?,
    )?;
    let mut block = Block {
        samples: budget.filled(count, Sample::default())?,
        width: p.width,
        height: p.height,
        orientation: p.orientation,
        causal: p.modes & segments::CAUSAL != 0,
    };
    let mut contexts = Decoder::initial_contexts();
    for segment in segments {
        decode_segment(&mut block, segment, p, &mut contexts)?;
    }
    Ok(block.samples)
}

fn decode_segment_inner(
    block: &mut Block,
    segment: &Segment,
    p: &Parameters,
    contexts: &mut [Context; 19],
) -> Result<()> {
    // B.10.6/D.4: a final layer may end at a nonterminated coding pass.
    let end = segment.first + segment.passes;
    let terminated =
        end == usize::from(p.planes) * 3 - 2 || end == segments::end(segment.first, p.modes);
    let predictable = terminated && p.modes & segments::PREDICTABLE != 0;
    let mut decoder = if segments::raw(segment.first, p.modes) {
        Entropy::Raw(Raw {
            data: &segment.data,
            offset: 0,
            base: 0,
            byte: 0,
            remaining: 0,
            synthetic: false,
        })
    } else {
        Entropy::Mq(Decoder::new(&segment.data, 0, *contexts, predictable)?)
    };
    for pass in segment.first..segment.first + segment.passes {
        let plane = p.planes
            - 1
            - u8::try_from(pass.div_ceil(3))
                .map_err(|_| syntax("JPEG 2000 bit plane overflow", 0))?;
        match pass % 3 {
            1 => block.propagation(&mut decoder, plane)?,
            2 => block.refinement(&mut decoder, plane)?,
            _ => {
                block.cleanup(&mut decoder, plane)?;
                if p.modes & segments::SYMBOLS != 0 {
                    let mut symbol = 0;
                    for _ in 0..4 {
                        symbol = (symbol << 1) | u8::from(decoder.bit(18)?);
                    }
                    if symbol != 10 {
                        return Err(syntax("invalid JPEG 2000 cleanup segmentation symbol", 0));
                    }
                }
            }
        }
        if p.modes & segments::RESET != 0 {
            *contexts = Decoder::initial_contexts();
            if let Entropy::Mq(decoder) = &mut decoder {
                decoder.reset();
            }
        }
    }
    match decoder {
        Entropy::Mq(decoder) => {
            *contexts = decoder.contexts();
            decoder.finish()?;
        }
        Entropy::Raw(decoder) => decoder.finish(predictable)?,
    }
    Ok(())
}

fn decode_segment(
    block: &mut Block,
    segment: &Segment,
    p: &Parameters,
    contexts: &mut [Context; 19],
) -> Result<()> {
    decode_segment_inner(block, segment, p, contexts).map_err(|mut error| {
        if let pagura_core::Position::Buffer(offset) = error.position {
            error.position = pagura_core::Position::Buffer(super::io::source_position(
                &segment.spans,
                offset,
                segment.base,
            ));
        }
        error
    })
}
