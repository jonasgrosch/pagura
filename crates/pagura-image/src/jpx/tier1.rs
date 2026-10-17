//! Embedded code-block bit planes (ISO/IEC 15444-1 Annex D).

use pagura_core::Result;

use super::io::{Budget, syntax};
use super::mq::Decoder;

#[derive(Clone, Copy, Default)]
struct Sample {
    magnitude: u32,
    negative: bool,
    visited: bool,
    refined: bool,
}

struct Block {
    samples: Vec<Sample>,
    width: usize,
    height: usize,
    orientation: u8,
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
                if dx == 0 && dy == 0 {
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

    fn sign(&self, decoder: &mut Decoder<'_>, x: usize, y: usize) -> Result<bool> {
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
        Ok(decoder.bit(context)? ^ prediction)
    }

    fn make_significant(
        &mut self,
        decoder: &mut Decoder<'_>,
        x: usize,
        y: usize,
        plane: u8,
    ) -> Result<()> {
        let negative = self.sign(decoder, x, y)?;
        let sample = &mut self.samples[y * self.width + x];
        sample.magnitude = 1 << plane;
        sample.negative = negative;
        Ok(())
    }

    fn propagation(&mut self, decoder: &mut Decoder<'_>, plane: u8) -> Result<()> {
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

    fn refinement(&mut self, decoder: &mut Decoder<'_>, plane: u8) -> Result<()> {
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
                }
            }
        }
        Ok(())
    }

    fn cleanup(&mut self, decoder: &mut Decoder<'_>, plane: u8) -> Result<()> {
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
    pub reset: bool,
    pub segmentation_symbols: bool,
}

pub(super) fn decode(
    data: &[u8],
    parameters: &Parameters,
    base: usize,
    budget: &mut Budget<'_>,
) -> Result<Vec<i64>> {
    let p = parameters;
    if p.planes == 0 || p.planes > 30 || p.passes != usize::from(p.planes) * 3 - 2 {
        return Err(syntax("invalid or incomplete JPEG 2000 bit planes", base));
    }
    let count = p.width * p.height; // Code-block dimensions are validated and at most 1024 each.
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
    };
    let mut decoder = Decoder::new(data, base)?;
    for pass in 0..p.passes {
        let plane = p.planes
            - 1
            - u8::try_from(pass.div_ceil(3))
                .map_err(|_| syntax("JPEG 2000 bit plane overflow", base))?;
        match pass % 3 {
            1 => block.propagation(&mut decoder, plane)?,
            2 => block.refinement(&mut decoder, plane)?,
            _ => {
                block.cleanup(&mut decoder, plane)?;
                if p.segmentation_symbols {
                    let mut symbol = 0;
                    for _ in 0..4 {
                        symbol = (symbol << 1) | u8::from(decoder.bit(18)?);
                    }
                    if symbol != 10 {
                        return Err(syntax(
                            "invalid JPEG 2000 cleanup segmentation symbol",
                            base,
                        ));
                    }
                }
            }
        }
        if p.reset {
            decoder.reset();
        }
    }
    let mut result = budget.filled(count, 0_i64)?;
    for (out, sample) in result.iter_mut().zip(block.samples) {
        *out = i64::from(sample.magnitude) * if sample.negative { -1 } else { 1 };
    }
    Ok(result)
}
