//! Stream bytes retain either an immutable source lease or decoded storage.

use crate::source::ByteLease;
use std::ops::Deref;

#[derive(Debug)]
enum Storage {
    Source(ByteLease),
    Decoded(Vec<u8>),
}

/// Stream data independent of the document's lifetime.
///
/// Unchanged bytes retain their source lease and allocation charge. Decoding or
/// decryption owns its output. Reading the byte view performs no source read or
/// copy; an issued value stays readable after its document or source closes.
#[derive(Debug)]
pub struct StreamData(Storage);

impl StreamData {
    /// Storage owned here: inline metadata and any decoded vector capacity.
    /// The shared source backing remains charged to its source reservation.
    pub fn retained_bytes(&self) -> usize {
        size_of::<Self>().saturating_add(match &self.0 {
            Storage::Source(_) => 0,
            Storage::Decoded(bytes) => bytes.capacity(),
        })
    }

    /// Transfers decoded storage, or explicitly copies a source lease's bytes.
    /// The returned vector is independent of the source's allocation charge.
    pub fn into_owned(self) -> Vec<u8> {
        match self.0 {
            Storage::Source(lease) => lease.bytes().to_vec(),
            Storage::Decoded(bytes) => bytes,
        }
    }
}

impl AsRef<[u8]> for StreamData {
    fn as_ref(&self) -> &[u8] {
        match &self.0 {
            Storage::Source(lease) => lease.bytes(),
            Storage::Decoded(bytes) => bytes,
        }
    }
}

impl Deref for StreamData {
    type Target = [u8];
    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

impl From<ByteLease> for StreamData {
    fn from(lease: ByteLease) -> Self {
        Self(Storage::Source(lease))
    }
}

impl From<Vec<u8>> for StreamData {
    fn from(bytes: Vec<u8>) -> Self {
        Self(Storage::Decoded(bytes))
    }
}
