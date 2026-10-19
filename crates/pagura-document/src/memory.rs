//! Aggregate retained-cache accounting (ADR 0012).

use pagura_core::syntax::PathComponent;
use pagura_core::{INDEX_ENTRY_OVERHEAD, Meter, ObjectId, Result, ResultExt};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// Estimated bytes of one object-table entry, including its tree bookkeeping.
pub(crate) const fn slot_bytes() -> usize {
    size_of::<(ObjectId, crate::Slot)>().saturating_add(INDEX_ENTRY_OVERHEAD)
}

/// Estimated bytes of one object-stream cache entry, including its tree
/// bookkeeping.
pub(crate) const fn stream_slot_bytes() -> usize {
    size_of::<(
        u32,
        std::sync::OnceLock<Result<crate::object::stream::Contents>>,
    )>()
    .saturating_add(INDEX_ENTRY_OVERHEAD)
}

/// Bytes a stored source path retains beyond its own `Vec` header: the buffer
/// of `capacity` components and each component's heap data.
pub(crate) fn path_bytes(path: &[PathComponent], capacity: usize) -> usize {
    path.iter().fold(
        capacity.saturating_mul(size_of::<PathComponent>()),
        |sum, part| {
            sum.saturating_add(
                part.retained_bytes()
                    .saturating_sub(size_of::<PathComponent>()),
            )
        },
    )
}

const EXCEEDED: &str = "document retained-memory budget exceeded";

/// The retained-cache budget of one document, shared by every thread that
/// loads objects. A lock guards one [`Meter`]; it is held only for the
/// arithmetic of a charge or release, never while loading.
#[derive(Debug)]
pub(crate) struct Budget {
    meter: Mutex<Meter>,
    /// The source document's budget, for a revision snapshot: every charge
    /// counts against both, so snapshots share one aggregate (ADR 0012).
    parent: Option<Arc<Budget>>,
}

impl Budget {
    pub(crate) const fn new(limit: usize) -> Self {
        Self {
            meter: Mutex::new(Meter::new(limit, EXCEEDED)),
            parent: None,
        }
    }

    /// A snapshot's budget, with its own account inside `parent`'s.
    pub(crate) fn child(parent: Arc<Self>) -> Self {
        let limit = parent.meter().max();
        Self {
            meter: Mutex::new(Meter::new(limit, EXCEEDED)),
            parent: Some(parent),
        }
    }

    /// Charges `bytes`, attributed to `object` when they belong to one rather
    /// than to the document's structure.
    pub(crate) fn charge(&self, bytes: usize, object: Option<ObjectId>) -> Result<()> {
        // The lock order is always child before parent.
        let mut meter = self.meter();
        meter.charge(bytes).in_object_opt(object)?;
        if let Some(parent) = &self.parent
            && let Err(error) = parent.charge(bytes, object)
        {
            meter.refund(bytes);
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn release(&self, bytes: usize) {
        let released = {
            let mut meter = self.meter();
            let released = bytes.min(meter.used());
            meter.refund(released);
            released
        };
        if let Some(parent) = &self.parent {
            parent.release(released);
        }
    }

    pub(crate) const fn reservation(&self) -> Reservation<'_> {
        Reservation {
            budget: self,
            bytes: 0,
        }
    }

    /// Whether this is a source document's budget rather than a snapshot's.
    pub(crate) const fn is_root(&self) -> bool {
        self.parent.is_none()
    }

    fn meter(&self) -> MutexGuard<'_, Meter> {
        self.meter.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn used(&self) -> usize {
        self.meter().used()
    }

    /// Releases everything charged since the budget was at `mark`.
    pub(crate) fn rewind(&self, mark: usize) {
        let since = self.used().saturating_sub(mark);
        self.release(since);
    }
}

impl Drop for Budget {
    fn drop(&mut self) {
        if let Some(parent) = &self.parent {
            parent.release(
                self.meter
                    .get_mut()
                    .unwrap_or_else(PoisonError::into_inner)
                    .used(),
            );
        }
    }
}

/// Tentative charges, released on drop unless committed. Page walks own one
/// until their complete result is cached; dropping a failed walk releases only
/// its pages and resources, not the objects it loaded.
pub(crate) struct Reservation<'a> {
    budget: &'a Budget,
    bytes: usize,
}

impl Reservation<'_> {
    pub(crate) fn charge(&mut self, bytes: usize, object: ObjectId) -> Result<()> {
        self.budget.charge(bytes, Some(object))?;
        // Both terms are part of the checked, bounded aggregate above.
        self.bytes = self.bytes.saturating_add(bytes);
        Ok(())
    }

    /// Keeps every charge made so far, for the lifetime of the budget.
    pub(crate) fn commit(mut self) {
        self.bytes = 0;
    }
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        self.budget.release(self.bytes);
    }
}

#[cfg(test)]
mod tests;
