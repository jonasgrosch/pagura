//! Arithmetic integers and symbol identifiers (T.88 Annex A).
use super::arithmetic::{Context, Decoder};
use pagura_core::{Error, Result};

pub(super) struct Integer {
    contexts: [Context; 512],
}

impl Integer {
    pub(super) fn new() -> Self {
        Self {
            contexts: [Context::default(); 512],
        }
    }

    /// Decodes an integer, or `None` for the out-of-band value (T.88 A.2).
    pub(super) fn decode(&mut self, decoder: &mut Decoder<'_>) -> Option<i64> {
        let mut previous = 1;
        let sign = self.bit(decoder, &mut previous);
        let mut category = 0;
        while category < 5 && self.bit(decoder, &mut previous) != 0 {
            category += 1;
        }
        let (bits, offset) = [(2, 0), (4, 4), (6, 20), (8, 84), (12, 340), (32, 4436)][category];
        let mut value = 0i64;
        for _ in 0..bits {
            value = (value << 1) | i64::from(self.bit(decoder, &mut previous));
        }
        value += offset;
        if sign == 0 {
            Some(value)
        } else if value == 0 {
            None
        } else {
            Some(-value)
        }
    }

    /// Decodes an integer that must not be out-of-band.
    pub(super) fn required(&mut self, decoder: &mut Decoder<'_>) -> Result<i64> {
        self.decode(decoder)
            .ok_or_else(|| Error::syntax("unexpected JBIG2 integer out-of-band"))
    }

    fn bit(&mut self, decoder: &mut Decoder<'_>, previous: &mut usize) -> u8 {
        let bit = decoder.bit(&mut self.contexts[*previous]);
        *previous = if *previous < 256 {
            (*previous << 1) | usize::from(bit)
        } else {
            (((*previous << 1) | usize::from(bit)) & 511) | 256
        };
        bit
    }
}

/// Decodes a symbol identifier with `contexts`, one per node of its binary
/// tree (T.88 A.3).
pub(super) fn symbol_id(decoder: &mut Decoder<'_>, contexts: &mut [Context]) -> usize {
    let mut previous = 1;
    while previous < contexts.len() {
        previous = (previous << 1) | usize::from(decoder.bit(&mut contexts[previous]));
    }
    previous - contexts.len()
}
