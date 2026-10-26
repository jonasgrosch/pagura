//! Shared allocation reservations whose charge follows the final owner.

use crate::{Meter, Result};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// A domain's retained-memory budget, shared by its independent allocations.
///
/// Keep each reservation with its allocation, usually inside the same `Arc`.
/// Closing a producer then releases only storage that no consumer still owns.
/// Accounting estimates allocation capacity, not allocator overhead or RSS.
#[derive(Debug)]
pub struct MemoryBudget {
    meter: Mutex<Meter>,
    _admission: Option<Reservation>,
}

impl MemoryBudget {
    /// Creates a shared budget with the fixed limit-error message of its domain.
    pub fn new(limit: usize, message: &'static str) -> Arc<Self> {
        Arc::new(Self {
            meter: Mutex::new(Meter::new(limit, message)),
            _admission: None,
        })
    }

    /// Admits a child domain's entire ceiling before creating its budget.
    /// The admission stays charged to this parent until the child and its last
    /// allocation reservation drop. Child `used()` reports actual charges;
    /// parent `used()` includes admitted ceilings, not just occupied bytes.
    /// Counter/controller overhead is excluded from both counts.
    ///
    /// # Errors
    /// Returns the parent's `Limit` error without creating a child when its
    /// remaining admission cannot fit `limit`.
    pub fn admit(self: &Arc<Self>, limit: usize, message: &'static str) -> Result<Arc<Self>> {
        let admission = self.reserve(limit)?;
        Ok(Arc::new(Self {
            meter: Mutex::new(Meter::new(limit, message)),
            _admission: Some(admission),
        }))
    }

    fn meter(&self) -> MutexGuard<'_, Meter> {
        self.meter.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Bytes charged to all live reservations.
    pub fn used(&self) -> usize {
        self.meter().used()
    }

    /// Reserves an allocation's known bytes before allocating it.
    ///
    /// # Errors
    /// Returns `Limit` without changing the budget if the charge would exceed it.
    pub fn reserve(self: &Arc<Self>, bytes: usize) -> Result<Reservation> {
        self.meter().charge(bytes)?;
        Ok(Reservation {
            budget: Arc::clone(self),
            bytes,
        })
    }
}

/// One owned memory charge, refunded when the allocation's final owner drops it.
#[derive(Debug)]
pub struct Reservation {
    budget: Arc<MemoryBudget>,
    bytes: usize,
}

impl Reservation {
    /// The current charge of this allocation.
    pub const fn bytes(&self) -> usize {
        self.bytes
    }

    /// Reserves vector capacity and charges growth, including replacement overlap.
    /// The vector's existing capacity must belong to this reservation.
    ///
    /// # Errors
    /// Returns `Limit` if the budget or allocator refuses the growth. A failed
    /// allocator request keeps its attempted charge until this reservation drops.
    pub fn reserve<T>(&mut self, values: &mut Vec<T>, additional: usize) -> Result<()> {
        let mut meter = self.budget.meter();
        let before = meter.used();
        let result = meter.reserve_capacity(values, additional);
        self.bytes = self
            .bytes
            .saturating_add(meter.used().saturating_sub(before));
        result
    }

    /// Charges additional known bytes before the allocation grows.
    ///
    /// # Errors
    /// Returns `Limit` without changing either charge if the growth does not fit.
    pub fn grow(&mut self, bytes: usize) -> Result<()> {
        self.budget.meter().charge(bytes)?;
        self.bytes = self.bytes.saturating_add(bytes);
        Ok(())
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.budget.meter().refund(self.bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Error, ErrorKind};

    #[test]
    fn child_admission_follows_the_last_allocation_and_cannot_exceed_either_limit() {
        let parent = MemoryBudget::new(100, "aggregate admission exceeded");
        let child = parent.admit(80, "child allocation exceeded").unwrap();
        assert_eq!((parent.used(), child.used()), (80, 0));
        assert_eq!(
            parent.admit(21, "another child").unwrap_err(),
            Error::limit("aggregate admission exceeded")
        );
        let lease = child.reserve(70).unwrap();
        assert_eq!(
            child.reserve(11).unwrap_err(),
            Error::limit("child allocation exceeded")
        );
        assert_eq!(parent.reserve(21).unwrap_err().kind, ErrorKind::Limit);
        assert_eq!((parent.used(), child.used()), (80, 70));
        drop(child);
        assert_eq!(parent.used(), 80);
        drop(lease);
        assert_eq!(parent.used(), 0);
        let empty = parent.admit(0, "empty child").unwrap();
        assert_eq!(empty.reserve(1).unwrap_err().kind, ErrorKind::Limit);
        drop(empty);
        assert_eq!(parent.used(), 0);
    }
}
