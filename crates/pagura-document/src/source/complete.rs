use super::memory::{Budget, Retained, Storage};
use super::{ByteLease, ByteRange, ByteSource, ReadResult, SourceIdentity, SourceLimits};
use pagura_core::{Error, Limits, Meter, Result};
use std::sync::{Arc, Mutex, PoisonError};

/// A complete source sharing caller-owned immutable bytes without copying them.
#[derive(Debug)]
pub struct CompleteBytes {
    identity: SourceIdentity,
    length: u64,
    backing: Arc<Retained<Storage>>,
    budget: Arc<Budget>,
    work: Mutex<Meter>,
}

impl CompleteBytes {
    /// Admits complete immutable bytes under file and source storage limits.
    ///
    /// # Errors
    /// Returns `Limit` before publication if the file or allocation charge is too large.
    pub fn new(
        identity: SourceIdentity,
        bytes: Arc<[u8]>,
        limits: &Limits,
        source_limits: &SourceLimits,
    ) -> Result<Self> {
        let length =
            u64::try_from(bytes.len()).map_err(|_| Error::limit("source length overflow"))?;
        super::length(length, limits)?;
        let budget = Budget::new(source_limits.retained_bytes);
        let retained = bytes
            .len()
            .checked_add(size_of::<Retained<Storage>>() + 4 * size_of::<usize>())
            .ok_or(Error::limit("source retained size overflow"))?;
        let reservation = budget.reserve(retained)?;
        Ok(Self {
            identity,
            length,
            backing: Retained::new(Storage::Shared(bytes), reservation),
            budget,
            work: Mutex::new(Meter::new(
                source_limits.work,
                "source work budget exceeded",
            )),
        })
    }
}

impl ByteSource for CompleteBytes {
    fn identity(&self) -> SourceIdentity {
        self.identity
    }
    fn len(&self) -> u64 {
        self.length
    }
    fn generation(&self) -> u64 {
        0
    }
    fn retained_bytes(&self) -> usize {
        self.budget.used()
    }
    fn read_exact(&self, range: ByteRange) -> ReadResult<ByteLease> {
        let extent = range.offsets(self.length)?;
        self.work
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .charge(1)?;
        Ok(ByteLease {
            backing: Some(Arc::clone(&self.backing)),
            extent,
            range,
        })
    }
}
