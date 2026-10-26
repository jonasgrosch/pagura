use super::memory::{Budget, Retained, Storage};
use super::{ByteLease, ByteRange, ByteSource, ReadResult, SourceIdentity, SourceLimits};
use pagura_core::{Error, Limits, Meter, Result};
use std::sync::{Arc, Mutex, PoisonError};

/// A complete source sharing caller-owned immutable bytes without copying them.
#[derive(Debug)]
pub struct CompleteBytes {
    identity: SourceIdentity,
    lease: ByteLease,
    budget: Arc<Budget>,
    work: Mutex<Meter>,
}

impl CompleteBytes {
    /// Complete convenience input has a document-local identity and is admitted
    /// under file_bytes rather than the registered source-residency default.
    pub(super) fn local(bytes: Arc<[u8]>, limits: &Limits) -> Result<Self> {
        let (lease, budget) = admit(bytes, limits, None)?;
        Ok(Self {
            identity: SourceIdentity::new(0, [0; 32]),
            lease,
            budget,
            work: Mutex::new(Meter::new(
                SourceLimits::default().work,
                "source work budget exceeded",
            )),
        })
    }

    pub(super) fn complete_lease(&self) -> ByteLease {
        self.lease.clone()
    }

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
        let (lease, budget) = admit(bytes, limits, Some(source_limits.retained_bytes))?;
        Ok(Self {
            identity,
            lease,
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
        self.lease.range().end()
    }
    fn generation(&self) -> u64 {
        0
    }
    fn retained_bytes(&self) -> usize {
        self.budget.used()
    }
    fn check(&self) -> Result<()> {
        Ok(())
    }
    fn read_exact(&self, range: ByteRange) -> ReadResult<ByteLease> {
        let lease = self.lease.slice(range)?;
        self.work
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .charge(1)?;
        Ok(lease)
    }
}

/// One storage admission for registered complete sources and document-local
/// complete input. Local input is bounded by file_bytes; its exact allocation
/// plus fixed metadata becomes the reservation cap, preserving that API limit.
pub(super) fn admit(
    bytes: Arc<[u8]>,
    limits: &Limits,
    retained_limit: Option<usize>,
) -> Result<(ByteLease, Arc<Budget>)> {
    let length = u64::try_from(bytes.len()).map_err(|_| Error::limit("source length overflow"))?;
    super::length(length, limits)?;
    let retained = bytes
        .len()
        .checked_add(size_of::<Retained<Storage>>() + 4 * size_of::<usize>())
        .ok_or(Error::limit("source retained size overflow"))?;
    let budget = Budget::new(retained_limit.unwrap_or(retained), super::memory::EXCEEDED);
    let reservation = budget.reserve(retained)?;
    let extent = 0..bytes.len();
    let lease = ByteLease {
        backing: Some(Retained::new(Storage::Shared(bytes), reservation)),
        extent,
        range: ByteRange::whole(length),
    };
    Ok((lease, budget))
}
