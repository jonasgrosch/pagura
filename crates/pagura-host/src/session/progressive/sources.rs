//! Immutable source registration over the existing lease and budget owners.

use crate::ProgressiveLimits;
use crate::protocol::Field;
use pagura::source::{ByteSource, SourceIdentity, SparseBytes, Supply};
use pagura::{Error, Limits, MemoryBudget, Result};
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Debug)]
pub(super) struct Sources {
    entries: BTreeMap<u32, Arc<SparseBytes>>,
    next: u32,
    budget: Arc<MemoryBudget>,
}

impl Sources {
    pub(super) fn new(budget: Arc<MemoryBudget>) -> Self {
        Self {
            entries: BTreeMap::new(),
            next: 1,
            budget,
        }
    }

    pub(super) fn create(
        &mut self,
        length: Field<u64>,
        version: Field<[u8; 32]>,
        engine: &Limits,
        limits: &ProgressiveLimits,
    ) -> Result<u32> {
        if self.entries.len() >= limits.sources || self.next == u32::MAX {
            return Err(Error::limit("session source handle budget exceeded"));
        }
        let handle = self.next;
        let source = SparseBytes::new_with_budget(
            SourceIdentity::new(u64::from(handle), version.get()),
            length.get(),
            engine,
            &limits.source,
            &self.budget,
        )
        .map_err(|error| error.at(length.position()))?;
        self.entries.insert(handle, Arc::new(source));
        self.next += 1;
        Ok(handle)
    }

    pub(super) fn get(&self, handle: Field<u32>) -> Result<Arc<SparseBytes>> {
        self.entries
            .get(&handle.get())
            .cloned()
            .ok_or_else(|| Error::invalid_argument("unknown source handle").at(handle.position()))
    }

    pub(super) fn supply(
        &self,
        handle: Field<u32>,
        version: Field<[u8; 32]>,
        offset: Field<u64>,
        bytes: Field<&[u8]>,
        limits: &ProgressiveLimits,
    ) -> Result<Supply> {
        if bytes.get().len() > limits.supply_bytes {
            return Err(Error::limit("source supply request budget exceeded").at(bytes.position()));
        }
        let source = self.get(handle)?;
        let identity = SourceIdentity::new(u64::from(handle.get()), version.get());
        if identity != source.identity() {
            return Err(
                Error::invalid_argument("supply belongs to another source version")
                    .at(version.position()),
            );
        }
        source
            .supply(identity, offset.get(), bytes.get())
            .map_err(|error| error.at(offset.position()))
    }

    pub(super) fn close(&mut self, handle: Field<u32>) -> Result<()> {
        let source = self.entries.remove(&handle.get()).ok_or_else(|| {
            Error::invalid_argument("unknown source handle").at(handle.position())
        })?;
        source.close();
        Ok(())
    }

    pub(super) fn count(&self) -> usize {
        self.entries.len()
    }
    pub(super) fn admitted_bytes(&self) -> usize {
        self.budget.used()
    }
    pub(super) fn registered_bytes(&self) -> usize {
        self.entries
            .values()
            .map(|source| source.retained_bytes())
            .fold(0, usize::saturating_add)
    }
}

impl Drop for Sources {
    fn drop(&mut self) {
        for source in self.entries.values() {
            source.close();
        }
    }
}
