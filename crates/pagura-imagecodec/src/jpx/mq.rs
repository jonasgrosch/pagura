//! Arithmetic-coded JPEG 2000 code-block segments: the shared MQ decoder
//! (T.800 C.3) with the 19 tier-1 contexts and the D.4 termination checks.

use crate::mq::{self, Context};
use pagura_core::Result;

use super::segments::Segment;
use super::syntax;
use super::termination::Check;

/// The tier-1 contexts in their initial states (T.800 Table D.7): the
/// uniform context in state 46, the run-length context in state 3, the
/// all-zero-neighbourhood significance context in state 4, and the others
/// in state 0.
const INITIAL: [Context; 19] = {
    let mut contexts = [Context::with_state(0); 19];
    contexts[0] = Context::with_state(4);
    contexts[17] = Context::with_state(3);
    contexts[18] = Context::with_state(46);
    contexts
};

/// The smallest byte that makes a preceding 0xFF a marker rather than
/// arithmetic-coded data (T.800 C.3.4).
const MARKER: u8 = 0x90;

pub(super) struct Decoder<'a> {
    coder: mq::Decoder<'a>,
    segment: &'a Segment,
    contexts: [Context; 19],
    termination: Option<Check<'a>>,
}

impl<'a> Decoder<'a> {
    pub(super) fn new(segment: &'a Segment, contexts: [Context; 19], predictable: bool) -> Self {
        Self {
            coder: mq::Decoder::new(&segment.data),
            segment,
            contexts,
            termination: predictable.then(|| Check::new(segment)),
        }
    }

    pub(super) fn contexts(&self) -> [Context; 19] {
        self.contexts
    }

    pub(super) fn initial_contexts() -> [Context; 19] {
        INITIAL
    }

    pub(super) fn reset(&mut self) {
        self.contexts = INITIAL;
    }

    pub(super) fn finish(self) -> Result<()> {
        if let Some(check) = self.termination {
            check.finish()?;
        }
        // T.800 D.4 permits alternate ordinary flushes; it does not mandate a byte
        // pattern. A marker can only end the segment (T.800 C.3.4).
        for (offset, bytes) in self.segment.data.windows(2).enumerate() {
            if bytes[0] == 0xff && bytes[1] >= MARKER {
                return Err(syntax(
                    "marker inside JPEG 2000 arithmetic segment",
                    self.segment.position(offset),
                ));
            }
        }
        Ok(())
    }

    pub(super) fn bit(&mut self, context: usize) -> Result<bool> {
        let old = self.contexts[context];
        let value = self.coder.decide(&mut self.contexts[context]);
        if let Some(check) = &mut self.termination {
            check.decision(old.estimate(), value == old.mps())?;
        }
        Ok(value)
    }
}
