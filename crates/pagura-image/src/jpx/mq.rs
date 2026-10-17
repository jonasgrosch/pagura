//! Binary arithmetic decoding, T.800 C.3.1–C.3.7.

use pagura_core::Result;

use super::io::syntax;

// Probability estimates and transitions, T.800 Table C.2.
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
    mps: bool,
}

pub(super) struct Decoder<'a> {
    bytes: &'a [u8],
    position: usize,
    base: usize,
    a: u32,
    c: u32,
    bits: u8,
    contexts: [Context; 19],
    termination: Option<super::termination::Check<'a>>,
}

impl<'a> Decoder<'a> {
    pub fn new(
        bytes: &'a [u8],
        base: usize,
        contexts: [Context; 19],
        predictable: bool,
    ) -> Result<Self> {
        let mut decoder = Self {
            bytes,
            position: 0,
            base,
            a: 0x8000,
            c: u32::from(bytes.first().copied().unwrap_or(255)) << 16,
            bits: 0,
            contexts,
            termination: predictable.then(|| super::termination::Check::new(bytes)),
        };
        decoder.byte_in()?;
        decoder.c <<= 7;
        decoder.bits -= 7;
        Ok(decoder)
    }

    pub fn contexts(&self) -> [Context; 19] {
        self.contexts
    }

    pub fn initial_contexts() -> [Context; 19] {
        let mut contexts = [Context::default(); 19];
        contexts[0].state = 4;
        contexts[17].state = 3;
        contexts[18].state = 46;
        contexts
    }

    pub fn reset(&mut self) {
        self.contexts.fill(Context::default());
        self.contexts[0].state = 4;
        self.contexts[17].state = 3;
        self.contexts[18].state = 46;
    }

    fn byte_in(&mut self) -> Result<()> {
        let previous = self.bytes.get(self.position).copied().unwrap_or(255);
        let next = self.bytes.get(self.position + 1).copied().unwrap_or(255);
        if previous == 255 {
            if next > 143 {
                // D.4.1 supplies synthetic FF bytes; C.3.4 holds the marker pointer.
                self.c = self.c.wrapping_add(0xff00);
                self.bits = 8;
                if self.position + 1 < self.bytes.len() {
                    return Err(syntax(
                        "marker inside JPEG 2000 arithmetic segment",
                        self.base + self.position,
                    ));
                }
            } else {
                self.position += 1;
                self.c = self.c.wrapping_add(u32::from(next) << 9);
                self.bits = 7;
            }
        } else {
            self.position += 1;
            self.c = self.c.wrapping_add(u32::from(next) << 8);
            self.bits = 8;
        }
        Ok(())
    }

    pub fn finish(self) -> Result<()> {
        if let Some(check) = self.termination {
            check.finish()?;
        }
        // D.4 permits alternate ordinary flushes; it does not mandate a byte pattern.
        for (offset, bytes) in self.bytes.windows(2).enumerate() {
            if bytes[0] == 255 && bytes[1] > 143 {
                return Err(syntax(
                    "marker inside JPEG 2000 arithmetic segment",
                    self.base + offset,
                ));
            }
        }
        Ok(())
    }

    pub fn bit(&mut self, context: usize) -> Result<bool> {
        let old = self.contexts[context];
        let value = self.decision(context)?;
        if let Some(check) = &mut self.termination {
            check.decision(STATES[usize::from(old.state)].0, value == old.mps)?;
        }
        Ok(value)
    }

    fn decision(&mut self, context: usize) -> Result<bool> {
        let old = self.contexts[context];
        let (estimate, mps_next, lps_next, switch) = STATES[usize::from(old.state)];
        self.a -= estimate;
        let value;
        if self.c >> 16 < estimate {
            if self.a < estimate {
                value = old.mps;
                self.contexts[context].state = mps_next;
            } else {
                value = !old.mps;
                self.contexts[context].state = lps_next;
                self.contexts[context].mps ^= switch;
            }
            self.a = estimate;
        } else {
            self.c -= estimate << 16;
            if self.a & 0x8000 != 0 {
                return Ok(old.mps);
            }
            if self.a < estimate {
                value = !old.mps;
                self.contexts[context].state = lps_next;
                self.contexts[context].mps ^= switch;
            } else {
                value = old.mps;
                self.contexts[context].state = mps_next;
            }
        }
        while self.a < 0x8000 {
            if self.bits == 0 {
                self.byte_in()?;
            }
            self.a <<= 1;
            self.c <<= 1;
            self.bits -= 1;
        }
        Ok(value)
    }
}
