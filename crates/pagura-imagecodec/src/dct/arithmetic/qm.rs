//! T.81 D.2: interval subdivision, conditional exchange and byte input.
use super::table::STATES;
use pagura_core::{Error, Position, Result};

#[derive(Clone, Copy, Default)]
pub(super) struct Bin {
    index: u8,
    mps: bool,
}

pub(super) struct Qm<'a> {
    data: &'a [u8],
    offset: usize,
    terminal: bool,
    interval: u32,
    code: u32,
    available: u8,
    remaining: usize,
    initial_budget: usize,
}

impl<'a> Qm<'a> {
    pub(super) fn new(data: &'a [u8], offset: usize, budget: usize) -> Result<Self> {
        let mut coder = Self {
            data,
            offset,
            terminal: false,
            interval: 0x1_0000,
            code: 0,
            available: 0,
            remaining: budget,
            initial_budget: budget,
        };
        // D.2.7: two bytes in Cx; C-low starts empty.
        coder.code = coder.byte()? << 24;
        coder.code |= coder.byte()? << 16;
        Ok(coder)
    }

    pub(super) fn error(&self, message: &'static str) -> Error {
        Error::syntax(message).at(Position::Buffer(self.offset))
    }

    fn charge(&mut self) -> Result<()> {
        self.remaining = self.remaining.checked_sub(1).ok_or(
            Error::limit("JPEG arithmetic work budget exceeded").at(Position::Buffer(self.offset)),
        )?;
        Ok(())
    }

    fn byte(&mut self) -> Result<u32> {
        self.charge()?;
        if self.terminal {
            // D.2.6 supplies zeros after an actual marker, never after EOF.
            return Ok(0);
        }
        let value = *self
            .data
            .get(self.offset)
            .ok_or(self.error("truncated JPEG arithmetic data"))?;
        if value == 255 {
            let next = *self
                .data
                .get(self.offset + 1)
                .ok_or(self.error("truncated JPEG arithmetic marker"))?;
            if next != 0 {
                self.terminal = true;
                return Ok(0);
            }
            self.offset += 1;
        }
        self.offset += 1;
        Ok(u32::from(value))
    }

    pub(super) fn decision(&mut self, bin: &mut Bin) -> Result<bool> {
        let (qe, lps, mps, switch) = STATES[usize::from(bin.index)];
        self.interval -= qe;
        let upper = (self.code >> 16) >= self.interval;
        if !upper && self.interval >= 0x8000 {
            return Ok(bin.mps);
        }
        let decoded_lps = if upper {
            self.code -= self.interval << 16;
            let exchanged = self.interval < qe;
            self.interval = qe;
            !exchanged
        } else {
            self.interval < qe
        };
        let decision = bin.mps ^ decoded_lps;
        if decoded_lps {
            bin.index = lps;
            bin.mps ^= switch;
        } else {
            bin.index = mps;
        }
        while self.interval < 0x8000 {
            if self.available == 0 {
                self.code |= self.byte()? << 8;
                self.available = 8;
            }
            self.interval <<= 1;
            self.code <<= 1;
            self.available -= 1;
        }
        Ok(decision)
    }

    pub(super) fn fixed(&mut self) -> Result<bool> {
        // F.1.4.4.2 / G.1.3.1: a fresh, non-adapting estimate for every bit.
        self.decision(&mut Bin::default())
    }

    pub(super) fn used(&self) -> usize {
        self.initial_budget - self.remaining
    }

    pub(super) fn finish(&mut self) -> Result<usize> {
        // D.1.8/D.2.6: termination bytes and optional zero suffix precede
        // the marker. Huffman padding and surplus-symbol rules do not apply.
        while !self.terminal {
            self.byte()?;
        }
        Ok(self.offset)
    }

    pub(super) fn restart(&mut self, expected: u8) -> Result<()> {
        let mut offset = self.finish()?;
        let code = super::super::marker(self.data, &mut offset)?;
        if code != 0xd0 + expected {
            return Err(self.error("wrong JPEG arithmetic restart marker"));
        }
        let used = self.used();
        *self = Self::new(self.data, offset, self.remaining)?;
        self.initial_budget += used;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Bin, Qm};

    #[test]
    fn annex_k_4_known_answer_including_stuffing_and_zero_termination() {
        // T.81 K.4.1 / Tables K.7-K.8, one adaptive context.
        let encoded = [
            0x65, 0x5b, 0x51, 0x44, 0xf7, 0x96, 0x9d, 0x51, 0x78, 0x55, 0xbf, 0xff, 0x00, 0xfc,
            0x51, 0x84, 0xc7, 0xce, 0xf9, 0x39, 0x00, 0x28, 0x7d, 0x46, 0x70, 0x8e, 0xcb, 0xc0,
            0xf6, 0xff, 0xd9, 0x00,
        ];
        let expected = [
            0x00, 0x02, 0x00, 0x51, 0x00, 0x00, 0x00, 0xc0, 0x03, 0x52, 0x87, 0x2a, 0xaa, 0xaa,
            0xaa, 0xaa, 0x82, 0xc0, 0x20, 0x00, 0xfc, 0xd7, 0x9e, 0xf6, 0x74, 0xea, 0xab, 0xf7,
            0x69, 0x7e, 0xe7, 0x4c,
        ];
        let mut coder = Qm::new(&encoded, 0, 10_000).unwrap();
        let mut bin = Bin::default();
        for byte in expected {
            for shift in (0..8).rev() {
                assert_eq!(coder.decision(&mut bin).unwrap(), byte & (1 << shift) != 0);
            }
        }
        assert_eq!(coder.finish().unwrap(), 29);
    }

    #[test]
    fn marker_termination_does_not_authorize_eof_or_unbounded_refills() {
        assert!(Qm::new(&[], 0, 100).is_err());
        assert!(Qm::new(&[0xff], 0, 100).is_err());
        let mut coder = Qm::new(&[0xff, 0xd9], 0, 8).unwrap();
        let mut bin = Bin::default();
        // Cheap adaptive decisions are covered by the scan's block charge.
        for _ in 0..32 {
            coder.decision(&mut bin).unwrap();
        }
        // A fresh estimate forces regular renormalization even on marker zeros.
        for _ in 0..128 {
            if let Err(error) = coder.fixed() {
                assert_eq!(coder.used(), 8);
                assert_eq!(error.kind, pagura_core::ErrorKind::Limit);
                assert!(matches!(error.position, pagura_core::Position::Buffer(_)));
                return;
            }
        }
        panic!("unbounded arithmetic byte refills");
    }
}
