//! Shared bounded byte storage for output encoders.
use pagura_core::{Error, Meter, Position, Result};

pub(crate) fn limit(offset: usize) -> Error {
    Error::limit("encoding budget exceeded").at(Position::Buffer(offset))
}

/// Reserves bounded encoder storage during admission, before output begins.
pub(crate) fn reserved<T: Clone>(len: usize, value: T, meter: &mut Meter) -> Result<Vec<T>> {
    let mut values = Vec::new();
    meter.reserve_capacity(&mut values, len)?;
    values.resize(len, value);
    Ok(values)
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

    pub(crate) const fn from_reserved(data: Vec<u8>, cap: usize) -> Self {
        Self { data, cap }
    }

    pub(crate) fn reserved(cap: usize, meter: &mut Meter) -> Result<Self> {
        let mut data = Vec::new();
        meter.reserve_capacity(&mut data, cap)?;
        Ok(Self { data, cap })
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
