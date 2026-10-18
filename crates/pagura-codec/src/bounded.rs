//! Shared bounded byte storage for output encoders.
use pagura_core::{Error, Position, Result};

pub(crate) fn limit(offset: usize) -> Error {
    Error::limit("encoding budget exceeded").at(Position::Buffer(offset))
}

pub(crate) fn filled<T: Clone>(len: usize, value: T) -> Result<Vec<T>> {
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(len).map_err(|_| limit(0))?;
    bytes.resize(len, value);
    Ok(bytes)
}

pub(crate) struct Bytes {
    pub(crate) data: Vec<u8>,
    cap: usize,
}

impl Bytes {
    pub(crate) const fn new(cap: usize) -> Self {
        Self {
            data: Vec::new(),
            cap,
        }
    }

    pub(crate) fn extend(&mut self, bytes: &[u8]) -> Result<()> {
        let required = self
            .data
            .len()
            .checked_add(bytes.len())
            .filter(|&len| len <= self.cap)
            .ok_or_else(|| limit(self.data.len()))?;
        if required > self.data.capacity() {
            let target = required
                .max(self.data.capacity().saturating_mul(2).max(64))
                .min(self.cap);
            self.data
                .try_reserve_exact(target - self.data.len())
                .map_err(|_| limit(self.data.len()))?;
        }
        self.data.extend_from_slice(bytes);
        Ok(())
    }
}
