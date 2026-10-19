//! T.81 D.2: interval subdivision, conditional exchange and byte input.
use super::table::STATES;
use crate::dct::marker;
use pagura_core::bytes::Cursor;
use pagura_core::{Error, Result};

#[derive(Clone, Copy, Default)]
pub(super) struct Bin {
    index: u8,
    mps: bool,
}

/// The QM decoder of T.81 Annex D over one entropy-coded segment.
///
/// Its work needs no meter of its own: every decision belongs to a block,
/// and the scan charges each block before decoding it.
pub(super) struct Qm<'a> {
    bytes: Cursor<'a>,
    /// Whether a marker has been reached; the decoder then reads zeros.
    terminal: bool,
    interval: u32,
    code: u32,
    available: u8,
}

impl<'a> Qm<'a> {
    pub(super) fn new(bytes: Cursor<'a>) -> Result<Self> {
        let mut coder = Self {
            bytes,
            terminal: false,
            interval: 0x1_0000,
            code: 0,
            available: 0,
        };
        // T.81 D.2.7: two bytes in Cx; C-low starts empty.
        coder.code = coder.byte()? << 24;
        coder.code |= coder.byte()? << 16;
        Ok(coder)
    }

    pub(super) fn error(&self, message: &'static str) -> Error {
        self.bytes.error(message)
    }

    /// BYTEIN with the stuffing of T.81 D.2.6 and F.1.2.3: `FF 00` is a data
    /// byte `FF`; `FF` and any other byte is a marker, after which the
    /// decoder reads zeros. The end of the data is not a marker.
    fn byte(&mut self) -> Result<u32> {
        if self.terminal {
            return Ok(0);
        }
        let at = self.bytes.position();
        let value = self
            .bytes
            .u8()
            .map_err(|_| Error::syntax("truncated JPEG arithmetic data").at(at))?;
        if value == marker::PREFIX {
            match self.bytes.rest().first() {
                None => return Err(Error::syntax("truncated JPEG arithmetic marker").at(at)),
                Some(&marker::STUFFED) => self.bytes.skip(1)?,
                Some(_) => {
                    self.terminal = true;
                    // Stay on the marker, which ends the segment.
                    self.bytes.seek(self.bytes.offset() - 1)?;
                    return Ok(0);
                }
            }
        }
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
        // T.81 F.1.4.4.2 and G.1.3.1: a fresh, non-adapting estimate for every bit.
        self.decision(&mut Bin::default())
    }

    /// Reads to the marker that ends the segment (T.81 D.1.8, D.2.6): the
    /// termination bytes and an optional zero suffix precede it. Huffman
    /// padding and surplus-symbol rules do not apply. Returns the marker's
    /// offset.
    pub(super) fn finish(&mut self) -> Result<usize> {
        while !self.terminal {
            self.byte()?;
        }
        Ok(self.bytes.offset())
    }

    /// Reads restart marker `RST<expected>` and starts the next segment.
    pub(super) fn restart(&mut self, expected: u8) -> Result<()> {
        self.finish()?;
        let mut bytes = self.bytes.clone();
        let at = bytes.position();
        if crate::dct::read_marker(&mut bytes)? != marker::RST0 + expected {
            return Err(Error::syntax("wrong JPEG arithmetic restart marker").at(at));
        }
        *self = Self::new(bytes)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Bin, Qm};
    use pagura_core::bytes::Cursor;

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
        let mut coder = Qm::new(Cursor::new(&encoded)).unwrap();
        let mut bin = Bin::default();
        for byte in expected {
            for shift in (0..8).rev() {
                assert_eq!(coder.decision(&mut bin).unwrap(), byte & (1 << shift) != 0);
            }
        }
        assert_eq!(coder.finish().unwrap(), 29);
    }

    #[test]
    fn markers_end_segments_but_the_end_of_data_does_not() {
        assert!(Qm::new(Cursor::new(&[])).is_err());
        assert!(Qm::new(Cursor::new(&[0xff])).is_err());
        // After a marker every byte reads as zero, however many are needed.
        let mut coder = Qm::new(Cursor::new(&[0xff, 0xd9])).unwrap();
        let mut bin = Bin::default();
        for _ in 0..1000 {
            coder.decision(&mut bin).unwrap();
            coder.fixed().unwrap();
        }
        assert_eq!(coder.finish().unwrap(), 0);
        // Without a marker, decoding past the data is an error.
        let mut coder = Qm::new(Cursor::new(&[0x12, 0x34])).unwrap();
        assert!((0..64).any(|_| coder.fixed().is_err()));
    }
}
