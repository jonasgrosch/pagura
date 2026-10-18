//! Best-effort clearing of key material once it is no longer needed (ADR 0021).
//!
//! Safe Rust cannot promise that no copy survives. Moves, reallocation,
//! registers and stack spills are outside its control, and the optimizer may
//! drop a store that nothing reads; [`std::hint::black_box`] only discourages
//! that. Clearing shortens the time key bytes stay in freed memory. It is not a
//! security boundary, in line with the crate's other non-guarantees.

/// Overwrites `values` with their default (zero) value.
pub(crate) fn wipe<T: Copy + Default>(values: &mut [T]) {
    values.fill(T::default());
    std::hint::black_box(values);
}

/// A growable byte buffer that is cleared when dropped. It never reallocates
/// after [`Self::with_capacity`], so no uncleared copy is left behind.
pub(crate) struct SecretBytes(Vec<u8>);

impl SecretBytes {
    /// Reserves exactly `capacity` bytes, or returns `None` if allocation fails.
    pub(crate) fn with_capacity(capacity: usize) -> Option<Self> {
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(capacity).ok()?;
        Some(Self(bytes))
    }

    /// Takes ownership of `bytes`, which are cleared on drop.
    pub(crate) const fn from_vec(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    pub(crate) fn clear(&mut self) {
        wipe(&mut self.0);
        self.0.clear();
    }

    /// Appends `data` within the reserved capacity.
    pub(crate) fn extend_from_slice(&mut self, data: &[u8]) {
        debug_assert!(self.0.len() + data.len() <= self.0.capacity());
        self.0.extend_from_slice(data);
    }
}

impl std::ops::Deref for SecretBytes {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.0
    }
}

impl std::ops::DerefMut for SecretBytes {
    fn deref_mut(&mut self) -> &mut [u8] {
        &mut self.0
    }
}

impl Drop for SecretBytes {
    fn drop(&mut self) {
        wipe(&mut self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wiping_clears_values_and_buffers() {
        let mut words = [7_u32, 8, 9];
        wipe(&mut words);
        assert_eq!(words, [0; 3]);
        let mut bytes = SecretBytes::with_capacity(4).unwrap();
        bytes.extend_from_slice(b"key");
        assert_eq!(&*bytes, b"key");
        bytes.clear();
        assert_eq!(bytes.len(), 0);
        assert_eq!(&*SecretBytes::from_vec(b"pw".to_vec()), b"pw");
    }
}
