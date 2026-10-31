//! Sample bytes a decoded image may share with the source it was decoded
//! from.

use std::ops::Deref;
use std::sync::Arc;

/// Bytes that several owners read: the decoded data of an image source, and
/// the samples decoded from it when those are the data byte for byte. A
/// clone shares the bytes instead of copying them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SharedBytes(Arc<Vec<u8>>);

impl SharedBytes {
    /// `bytes`, kept as they are, without a copy.
    #[must_use]
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(Arc::new(bytes))
    }

    /// The bytes.
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }

    /// Bytes allocated for them. Every owner counts them, as if it held a
    /// copy of its own.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.0.capacity()
    }

    /// Whether `self` and `other` are the same bytes, not a copy.
    #[must_use]
    pub fn shares(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Deref for SharedBytes {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.0
    }
}

impl From<Vec<u8>> for SharedBytes {
    fn from(bytes: Vec<u8>) -> Self {
        Self::new(bytes)
    }
}
