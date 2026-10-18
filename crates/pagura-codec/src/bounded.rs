//! Budgeted byte storage shared by the decoders' output and the encoders,
//! and fallible allocation of fixed-size scratch buffers.
use pagura_core::{Error, Meter, Position, Result};

/// The `Limit` message when decoded bytes would exceed the caller's cap.
pub(crate) const DECODE_BUDGET: &str = "decoded stream budget exceeded";
/// The `Limit` message when an encoder's input, output or scratch would
/// exceed its budget.
pub(crate) const ENCODE_BUDGET: &str = "encoding budget exceeded";

/// The decoding budget error at `offset` in the decoder's input.
pub(crate) fn decode_limit(offset: usize) -> Error {
    Error::limit(DECODE_BUDGET).at(Position::Buffer(offset))
}

/// The encoding budget error. Encoders have no input position to report.
pub(crate) const fn encode_limit() -> Error {
    Error::limit(ENCODE_BUDGET)
}

const SCRATCH: &str = "codec scratch memory unavailable";

/// A vector of `len` copies of `value`, without aborting when the
/// allocation fails.
pub(crate) fn filled<T: Clone>(len: usize, value: T) -> Result<Vec<T>> {
    let mut values = with_capacity(len)?;
    values.resize(len, value);
    Ok(values)
}

/// An empty vector with room for `capacity` values, without aborting when
/// the allocation fails.
pub(crate) fn with_capacity<T>(capacity: usize) -> Result<Vec<T>> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(capacity)
        .map_err(|_| Error::limit(SCRATCH))?;
    Ok(values)
}

/// Bytes charged to a [`Meter`] before they are stored. Capacity doubles
/// from 64 bytes but never exceeds the budget, and allocation failure is a
/// `Limit` error rather than an abort.
#[derive(Debug)]
pub(crate) struct BoundedBytes {
    data: Vec<u8>,
    budget: Meter,
    message: &'static str,
}

impl BoundedBytes {
    /// Empty storage admitting `max` bytes; `message` is the `Limit` error's.
    pub(crate) const fn new(max: usize, message: &'static str) -> Self {
        Self {
            data: Vec::new(),
            budget: Meter::new(max, message),
            message,
        }
    }

    /// Whether `count` more bytes fit.
    pub(crate) const fn fits(&self, count: usize) -> bool {
        count <= self.budget.remaining()
    }

    /// The budget.
    pub(crate) const fn max(&self) -> usize {
        self.budget.max()
    }

    pub(crate) fn as_slice(&self) -> &[u8] {
        &self.data
    }

    pub(crate) fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.data
    }

    pub(crate) fn into_vec(self) -> Vec<u8> {
        self.data
    }

    /// Appends `bytes`.
    ///
    /// # Errors
    ///
    /// A `Limit` error, without a position, if they do not fit or cannot be
    /// allocated; nothing is appended then.
    pub(crate) fn extend(&mut self, bytes: &[u8]) -> Result<()> {
        self.grow(bytes.len())?;
        self.data.extend_from_slice(bytes);
        Ok(())
    }

    /// Appends one byte; see [`BoundedBytes::extend`].
    pub(crate) fn push(&mut self, byte: u8) -> Result<()> {
        self.grow(1)?;
        self.data.push(byte);
        Ok(())
    }

    /// Appends `length` bytes copied from offset `start` on, repeating the
    /// bytes this appends when the run overlaps them (a DEFLATE copy).
    /// `start` must be below the stored length; see [`BoundedBytes::extend`].
    pub(crate) fn repeat(&mut self, start: usize, length: usize) -> Result<()> {
        debug_assert!(start < self.data.len());
        self.grow(length)?;
        let mut remaining = length;
        while remaining != 0 {
            let count = remaining.min(self.data.len() - start);
            self.data.extend_from_within(start..start + count);
            remaining -= count;
        }
        Ok(())
    }

    fn grow(&mut self, count: usize) -> Result<()> {
        self.budget.charge(count)?;
        let required = self.budget.used();
        if required > self.data.capacity() {
            let target = required
                .max(self.data.capacity().saturating_mul(2).max(64))
                .min(self.budget.max());
            if self
                .data
                .try_reserve_exact(target - self.data.len())
                .is_err()
            {
                self.budget.refund(count);
                return Err(Error::limit(self.message));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{BoundedBytes, ENCODE_BUDGET};
    use pagura_core::{ErrorKind, Position};

    #[test]
    fn storage_stops_at_its_budget_and_refuses_without_appending() {
        let mut bytes = BoundedBytes::new(5, ENCODE_BUDGET);
        bytes.extend(b"abc").unwrap();
        bytes.push(b'd').unwrap();
        assert!(bytes.fits(1));
        assert!(!bytes.fits(2));
        let error = bytes.extend(b"ef").unwrap_err();
        assert_eq!(
            (error.kind, error.message, error.position),
            (ErrorKind::Limit, ENCODE_BUDGET, Position::Unknown)
        );
        bytes.push(b'e').unwrap();
        assert_eq!(bytes.into_vec(), b"abcde");
    }
}
