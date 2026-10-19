//! Predictable MQ termination, T.800 D.4.2 with the C.2 register conventions.
//!
//! Replay the decoded interval choices, retaining only the pending carry byte.
//! Unlike ordinary termination (which permits alternate flushes), predictable
//! termination specifies the complete byte string. Comparing it validates all
//! tail bits, even those never requested by the arithmetic decoder.

use pagura_core::Result;

use super::segments::Segment;
use super::syntax;

pub(super) struct Check<'a> {
    segment: &'a Segment,
    offset: usize,
    a: u32,
    c: u32,
    bits: u8,
    buffer: u32,
    sentinel: bool,
}

impl<'a> Check<'a> {
    pub(super) fn new(segment: &'a Segment) -> Self {
        Self {
            segment,
            offset: 0,
            a: 0x8000,
            c: 0,
            bits: 12,
            buffer: 0,
            sentinel: true,
        }
    }

    fn byte_out(&mut self) -> Result<()> {
        if self.buffer != 255 && self.c & 0x0800_0000 != 0 {
            self.buffer += 1;
        }
        if self.sentinel {
            self.sentinel = false;
        } else {
            if self.segment.data.get(self.offset).map(|&v| u32::from(v)) != Some(self.buffer) {
                return Err(syntax(
                    "invalid JPEG 2000 predictable MQ termination",
                    self.segment.position(self.offset),
                ));
            }
            self.offset += 1;
        }
        let shift = if self.buffer == 255 { 20 } else { 19 };
        self.buffer = (self.c >> shift) & 255;
        self.c &= (1 << shift) - 1;
        self.bits = if shift == 20 { 7 } else { 8 };
        Ok(())
    }

    pub(super) fn decision(&mut self, estimate: u32, mps: bool) -> Result<()> {
        self.a -= estimate;
        if mps && self.a >= 0x8000 {
            self.c += estimate;
            return Ok(());
        }
        if (self.a < estimate) == mps {
            self.a = estimate;
        } else {
            self.c += estimate;
        }
        while self.a < 0x8000 {
            self.a <<= 1;
            self.c <<= 1;
            self.bits -= 1;
            if self.bits == 0 {
                self.byte_out()?;
            }
        }
        Ok(())
    }

    pub(super) fn finish(mut self) -> Result<()> {
        let mut needed = 12_i16 - i16::from(self.bits);
        while needed > 0 {
            self.c <<= self.bits;
            self.byte_out()?;
            needed -= i16::from(self.bits);
        }
        if self.buffer != 255 {
            self.byte_out()?;
        }
        if self.offset != self.segment.data.len() {
            return Err(syntax(
                "extra JPEG 2000 predictable MQ termination bytes",
                self.segment.position(self.offset),
            ));
        }
        Ok(())
    }
}
