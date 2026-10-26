//! MQ binary arithmetic decoding, T.88 Annex E (probability states, Table E.1).
use pagura_core::{Error, Position, Result};

// (Qe, next MPS state, next LPS state, exchange MPS). These are the
// probability-machine parameters specified by T.88 Table E.1.
const STATES: [(u32, u8, u8, bool); 47] = [
    (0x5601, 1, 1, true),
    (0x3401, 2, 6, false),
    (0x1801, 3, 9, false),
    (0x0ac1, 4, 12, false),
    (0x0521, 5, 29, false),
    (0x0221, 38, 33, false),
    (0x5601, 7, 6, true),
    (0x5401, 8, 14, false),
    (0x4801, 9, 14, false),
    (0x3801, 10, 14, false),
    (0x3001, 11, 17, false),
    (0x2401, 12, 18, false),
    (0x1c01, 13, 20, false),
    (0x1601, 29, 21, false),
    (0x5601, 15, 14, true),
    (0x5401, 16, 14, false),
    (0x5101, 17, 15, false),
    (0x4801, 18, 16, false),
    (0x3801, 19, 17, false),
    (0x3401, 20, 18, false),
    (0x3001, 21, 19, false),
    (0x2801, 22, 19, false),
    (0x2401, 23, 20, false),
    (0x2201, 24, 21, false),
    (0x1c01, 25, 22, false),
    (0x1801, 26, 23, false),
    (0x1601, 27, 24, false),
    (0x1401, 28, 25, false),
    (0x1201, 29, 26, false),
    (0x1101, 30, 27, false),
    (0x0ac1, 31, 28, false),
    (0x09c1, 32, 29, false),
    (0x08a1, 33, 30, false),
    (0x0521, 34, 31, false),
    (0x0441, 35, 32, false),
    (0x02a1, 36, 33, false),
    (0x0221, 37, 34, false),
    (0x0141, 38, 35, false),
    (0x0111, 39, 36, false),
    (0x0085, 40, 37, false),
    (0x0049, 41, 38, false),
    (0x0025, 42, 39, false),
    (0x0015, 43, 40, false),
    (0x0009, 44, 41, false),
    (0x0005, 45, 42, false),
    (0x0001, 45, 43, false),
    (0x5601, 46, 46, false),
];

#[derive(Clone, Copy, Default)]
pub(super) struct Context {
    state: u8,
    mps: u8,
}

pub(super) struct Decoder<'a> {
    data: &'a [u8],
    offset: usize,
    base: usize,
    interval: u32,
    code: u32,
    bits: u32,
}

impl<'a> Decoder<'a> {
    pub(super) fn new(data: &'a [u8], base: usize) -> Result<Self> {
        // Annex E termination is mandatory for arithmetic-coded segments.
        if !data.ends_with(&[0xff, 0xac]) {
            return Err(Error::syntax("missing JBIG2 arithmetic terminator")
                .at(Position::Buffer(base + data.len())));
        }
        for (offset, pair) in data.windows(2).enumerate().take(data.len() - 2) {
            if pair[0] == 255 && pair[1] > 0x8f {
                return Err(Error::syntax("invalid JBIG2 arithmetic byte stuffing")
                    .at(Position::Buffer(base + offset + 1)));
            }
        }
        let mut decoder = Self {
            data,
            offset: 0,
            base,
            interval: 0x8000,
            code: u32::from(data[0]) << 16,
            bits: 0,
        };
        decoder.byte_in()?;
        decoder.code <<= 7;
        decoder.bits -= 7;
        Ok(decoder)
    }

    fn byte_in(&mut self) -> Result<()> {
        let current = self.data[self.offset];
        let next = *self.data.get(self.offset + 1).ok_or_else(|| {
            Error::syntax("truncated JBIG2 arithmetic data")
                .at(Position::Buffer(self.base + self.offset + 1))
        })?;
        if current == 255 && next > 0x8f {
            if next != 0xac || self.offset + 2 != self.data.len() {
                return Err(Error::syntax("invalid JBIG2 arithmetic marker")
                    .at(Position::Buffer(self.base + self.offset + 1)));
            }
            // The termination marker supplies the Annex E all-one tail.
            self.code = self.code.wrapping_add(0xff00);
            self.bits = 8;
        } else {
            self.offset += 1;
            let shift = if current == 255 { 9 } else { 8 };
            self.code = self.code.wrapping_add(u32::from(next) << shift);
            self.bits = if current == 255 { 7 } else { 8 };
        }
        Ok(())
    }

    /// The next decision in `context` (E.3.2).
    #[expect(
        clippy::inline_always,
        reason = "every pixel of a region takes this path; it shares the region loop's registers"
    )]
    #[inline(always)]
    pub(super) fn bit(&mut self, context: &mut Context) -> Result<u8> {
        let (qe, nmps, nlps, exchange) = STATES[usize::from(context.state)];
        self.interval -= qe;
        let bit;
        if self.code >> 16 < qe {
            if self.interval < qe {
                bit = context.mps;
                context.state = nmps;
            } else {
                bit = context.mps ^ 1;
                context.state = nlps;
                if exchange {
                    context.mps ^= 1;
                }
            }
            self.interval = qe;
        } else {
            self.code -= qe << 16;
            if self.interval >= 0x8000 {
                return Ok(context.mps);
            }
            if self.interval < qe {
                bit = context.mps ^ 1;
                context.state = nlps;
                if exchange {
                    context.mps ^= 1;
                }
            } else {
                bit = context.mps;
                context.state = nmps;
            }
        }
        // E.3.3, shifting as many bits at once as the interval needs and
        // the current byte holds; the interval is at least one.
        while self.interval < 0x8000 {
            if self.bits == 0 {
                self.byte_in()?;
            }
            let shift = (self.interval.leading_zeros() - 16).min(self.bits);
            self.interval <<= shift;
            self.code <<= shift;
            self.bits -= shift;
        }
        Ok(bit)
    }
}

#[cfg(test)]
mod tests {
    use super::{Context, Decoder};
    use pagura_testkit::fixture;
    #[test]
    fn h2_published_known_answer() {
        let data = fixture("jbig2", "arithmetic-h2-known-answer.mq");
        let expected = fixture("jbig2", "arithmetic-h2-known-answer.decisions");
        let mut decoder = Decoder::new(&data, 0).unwrap();
        let mut context = Context::default();
        let mut actual = Vec::new();
        for _ in &expected {
            let mut byte = 0;
            for _ in 0..8 {
                byte = (byte << 1) | decoder.bit(&mut context).unwrap();
            }
            actual.push(byte);
        }
        assert_eq!(actual, expected);
    }
}
