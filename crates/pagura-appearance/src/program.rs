//! A bounded content-stream writer.

use pagura_core::{Error, Limits, Result};

/// Content bytes under the smallest of the stream, decoded and document
/// byte limits. Capacity grows geometrically from 256 bytes, never past the
/// limit, so retained-storage estimates are reproducible.
#[derive(Debug)]
pub(crate) struct Program {
    bytes: Vec<u8>,
    limit: usize,
}

impl Program {
    pub(crate) fn new(limits: &Limits) -> Self {
        Self {
            bytes: Vec::new(),
            limit: limits
                .stream_bytes
                .min(limits.decoded_bytes)
                .min(limits.document_bytes),
        }
    }

    pub(crate) fn text(&mut self, text: &str) -> Result<()> {
        self.bytes(text.as_bytes())
    }

    pub(crate) fn bytes(&mut self, bytes: &[u8]) -> Result<()> {
        let total = self
            .bytes
            .len()
            .checked_add(bytes.len())
            .filter(|total| *total <= self.limit)
            .ok_or(Error::limit("appearance content budget exceeded"))?;
        if total > self.bytes.capacity() {
            let capacity = self
                .bytes
                .capacity()
                .max(256)
                .saturating_mul(2)
                .max(total)
                .min(self.limit);
            self.bytes
                .try_reserve_exact(capacity.saturating_sub(self.bytes.len()))
                .map_err(|_| Error::limit("appearance content allocation failed"))?;
        }
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }

    pub(crate) fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pagura_core::ErrorKind;

    #[test]
    fn output_stops_at_the_smallest_byte_limit_without_partial_writes() {
        let mut out = Program::new(&Limits {
            stream_bytes: 10,
            ..Default::default()
        });
        out.text("1234567890").unwrap();
        assert_eq!(out.text("x").unwrap_err().kind, ErrorKind::Limit);
        assert_eq!(out.finish(), b"1234567890");
    }

    #[test]
    fn capacity_grows_geometrically_from_256_bytes() {
        let mut out = Program::new(&Limits::default());
        out.text("q").unwrap();
        assert_eq!(out.bytes.capacity(), 512);
        out.bytes(&[b' '; 600]).unwrap();
        assert_eq!(out.bytes.capacity(), 1024);
    }
}
