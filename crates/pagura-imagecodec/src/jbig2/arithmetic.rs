//! Arithmetic-coded JBIG2 segments: the shared MQ decoder (T.88 Annex E)
//! behind the JBIG2 segment termination.
pub(super) use crate::mq::Context;
use pagura_core::bytes::Cursor;
use pagura_core::{Error, Position, Result};

/// The marker that ends arithmetic-coded data (T.88 E.3.4, 7.4.6.4).
pub(super) const TERMINATOR: [u8; 2] = [0xff, 0xac];

pub(super) struct Decoder<'a>(crate::mq::Decoder<'a>);

impl<'a> Decoder<'a> {
    /// Starts decoding the rest of `bytes`, which it consumes. The data must
    /// end with the `FF AC` marker, which supplies the decoder's final one
    /// bits, and contain no other marker.
    pub(super) fn new(bytes: &mut Cursor<'a>) -> Result<Self> {
        let start = bytes.position();
        let data = bytes.take_rest();
        let at = |offset: usize| Position::Buffer(offset).within(start);
        let Some(coded) = data.strip_suffix(&TERMINATOR) else {
            return Err(Error::syntax("missing JBIG2 arithmetic terminator").at(at(data.len())));
        };
        // Pairs up to and including the one ending at the terminator's FF.
        for (offset, pair) in data.windows(2).enumerate().take(coded.len()) {
            if let [0xff, next] = *pair
                && next > 0x8f
            {
                return Err(
                    Error::syntax("invalid JBIG2 arithmetic byte stuffing").at(at(offset + 1))
                );
            }
        }
        Ok(Self(crate::mq::Decoder::new(coded)))
    }

    /// Decodes one bit in `context`.
    pub(super) fn bit(&mut self, context: &mut Context) -> u8 {
        u8::from(self.0.decide(context))
    }
}

#[cfg(test)]
mod tests {
    use super::{Context, Decoder};
    use pagura_core::bytes::Cursor;
    use pagura_core::{ErrorKind, Position};
    use pagura_testkit::fixture;

    #[test]
    fn h2_published_known_answer() {
        let data = fixture("jbig2", "arithmetic-h2-known-answer.mq");
        let expected = fixture("jbig2", "arithmetic-h2-known-answer.decisions");
        let mut decoder = Decoder::new(&mut Cursor::new(&data)).unwrap();
        let mut context = Context::default();
        let mut actual = Vec::new();
        for _ in &expected {
            let mut byte = 0;
            for _ in 0..8 {
                byte = (byte << 1) | decoder.bit(&mut context);
            }
            actual.push(byte);
        }
        assert_eq!(actual, expected);
    }

    #[test]
    fn segments_end_with_the_only_marker() {
        for (data, message, offset) in [
            (&[0x12, 0x34][..], "missing JBIG2 arithmetic terminator", 12),
            (
                &[0x12, 0xff, 0x90, 0xff, 0xac],
                "invalid JBIG2 arithmetic byte stuffing",
                12,
            ),
            (
                &[0x12, 0xff, 0xff, 0xac],
                "invalid JBIG2 arithmetic byte stuffing",
                12,
            ),
        ] {
            let error = Decoder::new(&mut Cursor::new(data).with_base(10))
                .err()
                .unwrap();
            assert_eq!(
                (error.kind, error.message, error.position),
                (ErrorKind::Syntax, message, Position::Buffer(offset)),
                "{data:x?}"
            );
        }
    }
}
