//! The MQ binary arithmetic decoder shared by JBIG2 and JPEG 2000. T.88
//! Annex E and T.800 Annex C specify the same coder: the same probability
//! states, interval arithmetic and byte input. The formats differ only in
//! how a segment ends, which their callers check.

/// One row of the probability estimation table (T.88 Table E.1, T.800
/// Table C.2).
#[derive(Clone, Copy)]
struct State {
    /// The LPS probability estimate.
    qe: u32,
    /// The next state after an MPS renormalization.
    next_mps: u8,
    /// The next state after an LPS renormalization.
    next_lps: u8,
    /// Whether an LPS exchanges the sense of the MPS.
    switch: bool,
}

const fn state(qe: u32, next_mps: u8, next_lps: u8, switch: bool) -> State {
    State {
        qe,
        next_mps,
        next_lps,
        switch,
    }
}

/// T.88 Table E.1 = T.800 Table C.2.
const STATES: [State; 47] = [
    state(0x5601, 1, 1, true),
    state(0x3401, 2, 6, false),
    state(0x1801, 3, 9, false),
    state(0x0ac1, 4, 12, false),
    state(0x0521, 5, 29, false),
    state(0x0221, 38, 33, false),
    state(0x5601, 7, 6, true),
    state(0x5401, 8, 14, false),
    state(0x4801, 9, 14, false),
    state(0x3801, 10, 14, false),
    state(0x3001, 11, 17, false),
    state(0x2401, 12, 18, false),
    state(0x1c01, 13, 20, false),
    state(0x1601, 29, 21, false),
    state(0x5601, 15, 14, true),
    state(0x5401, 16, 14, false),
    state(0x5101, 17, 15, false),
    state(0x4801, 18, 16, false),
    state(0x3801, 19, 17, false),
    state(0x3401, 20, 18, false),
    state(0x3001, 21, 19, false),
    state(0x2801, 22, 19, false),
    state(0x2401, 23, 20, false),
    state(0x2201, 24, 21, false),
    state(0x1c01, 25, 22, false),
    state(0x1801, 26, 23, false),
    state(0x1601, 27, 24, false),
    state(0x1401, 28, 25, false),
    state(0x1201, 29, 26, false),
    state(0x1101, 30, 27, false),
    state(0x0ac1, 31, 28, false),
    state(0x09c1, 32, 29, false),
    state(0x08a1, 33, 30, false),
    state(0x0521, 34, 31, false),
    state(0x0441, 35, 32, false),
    state(0x02a1, 36, 33, false),
    state(0x0221, 37, 34, false),
    state(0x0141, 38, 35, false),
    state(0x0111, 39, 36, false),
    state(0x0085, 40, 37, false),
    state(0x0049, 41, 38, false),
    state(0x0025, 42, 39, false),
    state(0x0015, 43, 40, false),
    state(0x0009, 44, 41, false),
    state(0x0005, 45, 42, false),
    state(0x0001, 45, 43, false),
    state(0x5601, 46, 46, false),
];

/// The adaptive state of one context: an index into the probability table
/// and the value of the more probable symbol.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Context {
    state: u8,
    mps: bool,
}

impl Context {
    /// A context in probability state `state` with MPS zero, for initial
    /// states other than zero (T.800 Table D.7).
    pub(crate) const fn with_state(state: u8) -> Self {
        Self { state, mps: false }
    }

    /// The LPS probability estimate of the current state.
    pub(crate) const fn estimate(self) -> u32 {
        STATES[self.state as usize].qe
    }

    /// The current more probable symbol.
    pub(crate) const fn mps(self) -> bool {
        self.mps
    }
}

/// The software-conventions decoder of T.88 E.3 and T.800 C.3 over one
/// arithmetic-coded segment.
///
/// A byte `0xFF` followed by a byte above `0x8F` is a marker, and so is the
/// end of the segment: from there on the decoder supplies one bits
/// (T.88 E.3.4, T.800 C.3.4 and D.4.1). Callers check how their format
/// terminates a segment; decoding itself cannot fail.
pub(crate) struct Decoder<'a> {
    bytes: &'a [u8],
    /// The index of the byte last loaded into `c`.
    position: usize,
    a: u32,
    c: u32,
    /// Bits left before the next byte input (`CT`).
    ct: u32,
}

impl<'a> Decoder<'a> {
    /// INITDEC (T.88 E.3.5, T.800 C.3.5).
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        let mut decoder = Self {
            bytes,
            position: 0,
            a: 0x8000,
            c: u32::from(bytes.first().copied().unwrap_or(0xff)) << 16,
            ct: 0,
        };
        decoder.byte_in();
        decoder.c <<= 7;
        decoder.ct -= 7;
        decoder
    }

    /// BYTEIN (T.88 E.3.4, T.800 C.3.4).
    fn byte_in(&mut self) {
        let current = self.bytes.get(self.position).copied().unwrap_or(0xff);
        let next = self.bytes.get(self.position + 1).copied().unwrap_or(0xff);
        if current == 0xff && next > 0x8f {
            self.c = self.c.wrapping_add(0xff00);
            self.ct = 8;
        } else if current == 0xff {
            self.position += 1;
            self.c = self.c.wrapping_add(u32::from(next) << 9);
            self.ct = 7;
        } else {
            self.position += 1;
            self.c = self.c.wrapping_add(u32::from(next) << 8);
            self.ct = 8;
        }
    }

    /// DECODE (T.88 E.3.2, T.800 C.3.2): one decision in `context`, which
    /// adapts.
    pub(crate) fn decide(&mut self, context: &mut Context) -> bool {
        let State {
            qe,
            next_mps,
            next_lps,
            switch,
        } = STATES[usize::from(context.state)];
        let mps = context.mps;
        self.a -= qe;
        let lps = if self.c >> 16 < qe {
            // LPS_EXCHANGE: the subinterval sizes decide which symbol it is.
            let lps = self.a >= qe;
            self.a = qe;
            lps
        } else {
            self.c -= qe << 16;
            if self.a & 0x8000 != 0 {
                return mps;
            }
            // MPS_EXCHANGE.
            self.a < qe
        };
        if lps {
            context.state = next_lps;
            context.mps ^= switch;
        } else {
            context.state = next_mps;
        }
        // RENORMD.
        while self.a < 0x8000 {
            if self.ct == 0 {
                self.byte_in();
            }
            self.a <<= 1;
            self.c <<= 1;
            self.ct -= 1;
        }
        mps ^ lps
    }
}

#[cfg(test)]
mod tests {
    use super::{Context, Decoder};
    use pagura_testkit::fixture;

    #[test]
    fn decodes_the_published_h2_known_answer() {
        // T.88 H.2 (= T.800 Annex C test sequence), one context, with the
        // JBIG2 FF AC terminator removed as the JBIG2 caller does.
        let data = fixture("jbig2", "arithmetic-h2-known-answer.mq");
        let expected = fixture("jbig2", "arithmetic-h2-known-answer.decisions");
        let mut decoder = Decoder::new(data.strip_suffix(&[0xff, 0xac]).unwrap());
        let mut context = Context::default();
        let actual: Vec<u8> = expected
            .iter()
            .map(|_| {
                (0..8).fold(0, |byte, _| {
                    (byte << 1) | u8::from(decoder.decide(&mut context))
                })
            })
            .collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn supplies_one_bits_after_a_marker_and_after_the_end() {
        // Both inputs end the segment immediately: an empty segment and one
        // that starts with a marker decode the same decisions.
        let decisions = |bytes: &[u8]| {
            let mut decoder = Decoder::new(bytes);
            let mut context = Context::with_state(46);
            (0..64)
                .map(|_| decoder.decide(&mut context))
                .collect::<Vec<_>>()
        };
        assert_eq!(decisions(&[]), decisions(&[0xff, 0xac, 0x12]));
        assert_eq!(Context::with_state(46).estimate(), 0x5601);
        assert!(!Context::default().mps());
    }
}
