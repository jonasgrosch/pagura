//! Aggregate retained-cache accounting (ADR 0012).

use pagura_core::{Error, ObjectId, Result};
use std::sync::{Arc, Mutex, PoisonError};

pub(crate) const fn slot_bytes() -> usize {
    size_of::<(ObjectId, crate::Slot)>().saturating_add(64)
}

pub(crate) const fn stream_slot_bytes() -> usize {
    size_of::<(
        u32,
        std::sync::OnceLock<Result<crate::object::stream::Contents>>,
    )>()
    .saturating_add(64)
}

#[derive(Debug)]
pub(crate) struct Budget {
    limit: usize,
    used: Mutex<usize>,
    parent: Option<Arc<Budget>>,
}

impl Budget {
    pub(crate) const fn new(limit: usize) -> Self {
        Self {
            limit,
            used: Mutex::new(0),
            parent: None,
        }
    }

    pub(crate) fn child(parent: Arc<Self>) -> Self {
        Self {
            limit: parent.limit,
            used: Mutex::new(0),
            parent: Some(parent),
        }
    }

    pub(crate) fn charge(&self, bytes: usize, object: ObjectId) -> Result<()> {
        let mut used = self.used.lock().unwrap_or_else(PoisonError::into_inner);
        let next = used
            .checked_add(bytes)
            .filter(|&next| next <= self.limit)
            .ok_or_else(|| {
                Error::limit("document retained-memory budget exceeded").in_object(object)
            })?;
        if let Some(parent) = &self.parent {
            parent.charge(bytes, object)?;
        }
        *used = next;
        Ok(())
    }

    pub(crate) fn release(&self, bytes: usize) {
        let mut used = self.used.lock().unwrap_or_else(PoisonError::into_inner);
        let released = bytes.min(*used);
        *used -= released;
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

    #[cfg(test)]
    pub(crate) fn used(&self) -> usize {
        *self.used.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for Budget {
    fn drop(&mut self) {
        if let Some(parent) = &self.parent {
            parent.release(*self.used.get_mut().unwrap_or_else(PoisonError::into_inner));
        }
    }
}

/// Page walks own tentative reservations until the complete result is cached.
/// Dropping a failed walk releases only its pages/resources, not loaded objects.
pub(crate) struct Reservation<'a> {
    budget: &'a Budget,
    bytes: usize,
}

impl Reservation<'_> {
    pub(crate) fn charge(&mut self, bytes: usize, object: ObjectId) -> Result<()> {
        self.budget.charge(bytes, object)?;
        // Both terms are part of the checked, bounded aggregate above.
        self.bytes = self.bytes.saturating_add(bytes);
        Ok(())
    }

    pub(crate) fn commit(&mut self) {
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
