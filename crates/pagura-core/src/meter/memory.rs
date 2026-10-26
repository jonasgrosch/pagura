//! Shared allocation reservations whose charge follows the final owner.

use crate::{Meter, Result};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// A domain's retained-memory budget, shared by its independent allocations.
///
/// Keep each reservation with its allocation, usually inside the same `Arc`.
/// Closing a producer then releases only storage that no consumer still owns.
/// Accounting estimates allocation capacity, not allocator overhead or RSS.
#[derive(Debug)]
pub struct MemoryBudget(Mutex<Meter>);

impl MemoryBudget {
    /// Creates a shared budget with the fixed limit-error message of its domain.
    pub fn new(limit: usize, message: &'static str) -> Arc<Self> {
        Arc::new(Self(Mutex::new(Meter::new(limit, message))))
    }

    fn meter(&self) -> MutexGuard<'_, Meter> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
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
