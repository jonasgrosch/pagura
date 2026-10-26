//! Range-backed documents and historical prefixes of one immutable source.

use super::{
    ByteLease, ByteRange, ByteSource, CompleteBytes, LoadingLimits, LoadingWork, ReadResult,
    SourceIdentity, Window,
};
use pagura_core::convert::usize_to_u64;
use pagura_core::{Error, Limits, Result};
use std::sync::{Arc, OnceLock};

#[derive(Debug)]
struct Backing {
    source: Arc<dyn ByteSource>,
    /// Populated only by explicit complete admission/materialization.
    complete: OnceLock<ByteLease>,
    limits: Limits,
    loading: LoadingLimits,
    work: Arc<LoadingWork>,
}

/// A fixed source prefix. Reads retain checked leases; this owner never
/// pretends missing bytes are an addressable slice. Complete materialization
/// is explicit, cached once, and shared by historical prefix views.
#[derive(Debug, Clone)]
pub(crate) struct View {
    backing: Arc<Backing>,
    end: usize,
}

impl View {
    pub(crate) fn local(bytes: Arc<[u8]>, limits: &Limits) -> Result<Self> {
        let source = Arc::new(CompleteBytes::local(bytes, limits)?);
        let lease = source.complete_lease();
        let loading = LoadingLimits::default();
        Self::admitted(
            source,
            lease,
            *limits,
            loading,
            Arc::new(LoadingWork::new(loading.work)),
        )
    }

    pub(crate) fn registered(
        source: Arc<dyn ByteSource>,
        limits: Limits,
        loading: LoadingLimits,
        work: Arc<LoadingWork>,
    ) -> Result<Self> {
        source.check()?;
        let end = super::length(source.len(), &limits)?;
        Ok(Self {
            backing: Arc::new(Backing {
                source,
                complete: OnceLock::new(),
                limits,
                loading,
                work,
            }),
            end,
        })
    }

    pub(crate) fn admitted(
        source: Arc<dyn ByteSource>,
        lease: ByteLease,
        limits: Limits,
        loading: LoadingLimits,
        work: Arc<LoadingWork>,
    ) -> Result<Self> {
        if lease.range().start() != 0 || lease.range().end() != source.len() {
            return Err(Error::invalid_argument(
                "materialized lease does not cover its source",
            ));
        }
        let view = Self::registered(source, limits, loading, work)?;
        view.backing.complete.get_or_init(|| lease);
        Ok(view)
    }

    pub(crate) fn loading(&self) -> LoadingLimits {
        self.backing.loading
    }

    pub(crate) fn work(&self) -> Arc<LoadingWork> {
        Arc::clone(&self.backing.work)
    }

    pub(crate) const fn len(&self) -> usize {
        self.end
    }

    /// Source closure/conflict takes precedence over already-cached reads.
    pub(crate) fn check(&self) -> Result<()> {
        self.backing.source.check()
    }

    pub(crate) fn materialize(&self) -> ReadResult<ByteLease> {
        let lease = self.lease(ByteRange::whole(usize_to_u64(self.end)))?;
        if usize_to_u64(self.end) == self.backing.source.len() {
            return Ok(self.backing.complete.get_or_init(|| lease).clone());
        }
        Ok(lease)
    }

    /// Borrows no document storage: the returned lease owns the exact range.
    pub(crate) fn lease(&self, range: ByteRange) -> ReadResult<ByteLease> {
        self.check()?;
        if range.end() > usize_to_u64(self.end) {
            return Err(Error::invalid_argument("source range outside document prefix").into());
        }
        match self.backing.complete.get() {
            Some(lease) => Ok(lease.slice(range)?),
            None => self.backing.source.read_exact(range),
        }
    }

    /// Admits a bounded structural scan under the shared loading-work account.
    /// Unavailable reads cost only the attempted read; bytes are charged before
    /// the caller scans an available lease.
    pub(crate) fn scan(&self, range: ByteRange) -> ReadResult<ByteLease> {
        self.check()?;
        self.backing.work.charge(1)?;
        let lease = self.lease(range)?;
        self.backing.work.charge(lease.bytes().len())?;
        Ok(lease)
    }

    pub(crate) fn window(&self, offset: usize) -> Result<Window> {
        Window::new_with_work(
            Arc::new(self.clone()),
            usize_to_u64(offset),
            self.backing.limits,
            self.backing.loading,
            Arc::clone(&self.backing.work),
        )
    }

    pub(crate) fn prefix(&self, end: usize) -> Result<Self> {
        self.check()?;
        if end > self.end {
            return Err(Error::syntax("revision prefix lies outside its source"));
        }
        Ok(Self {
            backing: Arc::clone(&self.backing),
            end,
        })
    }
}

impl ByteSource for View {
    fn identity(&self) -> SourceIdentity {
        self.backing.source.identity()
    }
    fn len(&self) -> u64 {
        usize_to_u64(self.end)
    }
    fn generation(&self) -> u64 {
        self.backing.source.generation()
    }
    fn retained_bytes(&self) -> usize {
        self.backing.source.retained_bytes()
    }
    fn check(&self) -> Result<()> {
        self.check()
    }
    fn read_exact(&self, range: ByteRange) -> ReadResult<ByteLease> {
        self.lease(range)
    }
    fn materialize(&self) -> ReadResult<ByteLease> {
        self.materialize()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn materialized_historical_prefix_keeps_absolute_extent_and_shared_storage() {
        let original: Arc<[u8]> = Arc::from(&b"older revision\nnewer revision\n"[..]);
        let view = View::local(Arc::clone(&original), &Limits::default()).unwrap();
        let prefix = view.prefix(15).unwrap();
        let lease = prefix.materialize().unwrap();
        assert_eq!(lease.bytes(), b"older revision\n");
        assert_eq!(lease.range(), ByteRange::new(0, 15).unwrap());
        assert_eq!(lease.bytes().as_ptr(), original.as_ptr());
        assert!(prefix.prefix(16).is_err());
        drop(view);
        drop(prefix);
        assert_eq!(lease.bytes(), b"older revision\n");
    }
}
