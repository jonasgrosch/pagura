//! Incrementally charged work and memory budgets (ARCHITECTURE.md
//! invariant 3: every traversal and decoder has an explicit budget).

use crate::{Error, Result};

mod memory;
mod tables;
pub use memory::{MemoryBudget, Reservation};

/// A budget that work or bytes are charged against before they are spent.
///
/// A meter is created from one [`Limits`](crate::Limits) field, or a value
/// derived from one, together with the fixed message of the `Limit` error it
/// returns. Charging uses checked arithmetic: the total never wraps and never
/// exceeds the maximum, and a refused charge leaves the meter unchanged.
///
/// ```
/// use pagura_core::{ErrorKind, Limits, Meter};
///
/// let limits = Limits::default();
/// let mut work = Meter::new(limits.decode_work, "image decode work budget exceeded");
/// work.charge(64)?;
/// assert_eq!(work.used(), 64);
///
/// let mut tiny = Meter::new(10, "test budget exceeded");
/// let error = tiny.charge_product(4, 3).unwrap_err();
/// assert_eq!((error.kind, error.message), (ErrorKind::Limit, "test budget exceeded"));
/// assert_eq!(tiny.used(), 0);
/// # Ok::<(), pagura_core::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Meter {
    used: usize,
    max: usize,
    message: &'static str,
}

impl Meter {
    /// Creates an empty meter that admits `max` units in total and reports
    /// `message` when a charge would exceed it.
    pub const fn new(max: usize, message: &'static str) -> Self {
        Self {
            used: 0,
            max,
            message,
        }
    }

    /// Charges `units`.
    ///
    /// # Errors
    ///
    /// Returns a `Limit` error with the meter's message if the total would
    /// exceed the maximum; the meter is then unchanged.
    pub fn charge(&mut self, units: usize) -> Result<()> {
        self.used = self
            .used
            .checked_add(units)
            .filter(|&used| used <= self.max)
            .ok_or(Error::limit(self.message))?;
        Ok(())
    }

    /// Charges `count × unit`, for example pixels times work per pixel.
    ///
    /// # Errors
    ///
    /// As [`Meter::charge`], including when the product overflows.
    pub fn charge_product(&mut self, count: usize, unit: usize) -> Result<()> {
        let units = count.checked_mul(unit).ok_or(Error::limit(self.message))?;
        self.charge(units)
    }

    /// Charges the bytes of `count` values of `T`.
    ///
    /// # Errors
    ///
    /// As [`Meter::charge_product`].
    pub fn charge_items<T>(&mut self, count: usize) -> Result<()> {
        self.charge_product(count, size_of::<T>())
    }

    /// Charges the bytes of `additional` values of `T`, then reserves room
    /// for them in `values` without aborting on allocation failure.
    ///
    /// The charge is made even when `values` already has spare capacity: the
    /// meter counts what the caller is about to store.
    ///
    /// # Errors
    ///
    /// As [`Meter::charge_items`], or a `Limit` error with the meter's
    /// message if the allocation fails (the charge then stays recorded).
    pub fn reserve<T>(&mut self, values: &mut Vec<T>, additional: usize) -> Result<()> {
        self.charge_items::<T>(additional)?;
        values
            .try_reserve_exact(additional)
            .map_err(|_| Error::limit(self.message))
    }

    /// Makes room for `additional` values, charging changes in capacity.
    /// The vector's existing capacity must already be charged. Growth
    /// includes the old and replacement allocations until reservation
    /// succeeds, then returns the old capacity to the budget. It doubles
    /// capacity, from at least as many values as `Vec` first allocates,
    /// when that fits, otherwise reserves only the needed room.
    ///
    /// # Errors
    ///
    /// As [`Self::reserve`], before changing the vector when the temporary
    /// overlap would exceed the budget.
    pub fn reserve_capacity<T>(&mut self, values: &mut Vec<T>, additional: usize) -> Result<()> {
        let need = values
            .len()
            .checked_add(additional)
            .ok_or(Error::limit(self.message))?;
        let capacity = values.capacity();
        if need <= capacity {
            return Ok(());
        }
        let smallest = match size_of::<T>() {
            1 => 8,
            2..=1024 => 4,
            _ => 1,
        };
        let preferred = need.max(capacity.saturating_mul(2)).max(smallest);
        let target = if preferred
            .checked_mul(size_of::<T>())
            .is_some_and(|bytes| bytes <= self.remaining())
        {
            preferred
        } else {
            need
        };
        self.charge_items::<T>(target)?;
        values
            .try_reserve_exact(target - values.len())
            .map_err(|_| Error::limit(self.message))?;
        self.refund(capacity.saturating_mul(size_of::<T>()));
        Ok(())
    }

    /// Returns `units` to the budget, for memory that has been released.
    /// Refunds saturate at zero.
    pub const fn refund(&mut self, units: usize) {
        self.used = self.used.saturating_sub(units);
    }

    /// Units charged so far.
    pub const fn used(&self) -> usize {
        self.used
    }

    /// Units that can still be charged.
    pub const fn remaining(&self) -> usize {
        self.max - self.used
    }

    /// The total the meter admits.
    pub const fn max(&self) -> usize {
        self.max
    }
}

#[cfg(test)]
mod tests {
    use super::Meter;
    use crate::ErrorKind;

    const MESSAGE: &str = "test budget exceeded";

    #[test]
    fn charges_are_checked_and_refused_charges_change_nothing() {
        let mut meter = Meter::new(10, MESSAGE);
        meter.charge(4).unwrap();
        meter.charge(6).unwrap();
        assert_eq!((meter.used(), meter.remaining(), meter.max()), (10, 0, 10));
        let error = meter.charge(1).unwrap_err();
        assert_eq!((error.kind, error.message), (ErrorKind::Limit, MESSAGE));
        assert_eq!(meter.used(), 10);
        meter.refund(3);
        assert_eq!(meter.remaining(), 3);
        meter.refund(usize::MAX);
        assert_eq!(meter.used(), 0);
        meter.charge(0).unwrap();
    }

    #[test]
    fn overflowing_totals_and_products_are_limit_errors() {
        let mut meter = Meter::new(usize::MAX, MESSAGE);
        meter.charge(usize::MAX).unwrap();
        assert_eq!(meter.charge(1).unwrap_err().kind, ErrorKind::Limit);
        let mut meter = Meter::new(usize::MAX, MESSAGE);
        assert_eq!(
            meter.charge_product(usize::MAX, 2).unwrap_err().kind,
            ErrorKind::Limit
        );
        assert_eq!(meter.used(), 0);
        meter.charge_items::<u64>(3).unwrap();
        assert_eq!(meter.used(), 24);
    }

    #[test]
    fn reservations_charge_before_allocating() {
        let mut meter = Meter::new(16, MESSAGE);
        let mut values: Vec<u32> = Vec::new();
        meter.reserve(&mut values, 4).unwrap();
        assert!(values.capacity() >= 4);
        assert_eq!(meter.used(), 16);
        let error = meter.reserve(&mut values, 1).unwrap_err();
        assert_eq!(error.message, MESSAGE);
        let mut huge = Meter::new(usize::MAX, MESSAGE);
        let error = huge.reserve(&mut values, usize::MAX / 8).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit);
    }

    #[test]
    fn capacity_growth_charges_the_old_and_replacement_buffers() {
        let mut meter = Meter::new(35, MESSAGE);
        let mut values = Vec::new();
        meter.reserve(&mut values, 4).unwrap();
        values.extend([1_u32, 2, 3, 4]);
        let capacity = values.capacity();
        // The final five words fit, but the old four must stay alive
        // until the replacement allocation has succeeded.
        let error = meter.reserve_capacity(&mut values, 1).unwrap_err();
        assert_eq!((error.kind, error.message), (ErrorKind::Limit, MESSAGE));
        assert_eq!(values, [1, 2, 3, 4]);
        assert_eq!((values.capacity(), meter.used()), (capacity, 16));
        assert!(meter.reserve_capacity(&mut values, usize::MAX).is_err());
        assert_eq!((values.capacity(), meter.used()), (capacity, 16));
    }

    #[test]
    fn growth_from_empty_reserves_a_few_values() {
        let mut meter = Meter::new(1024, MESSAGE);
        let mut values: Vec<u32> = Vec::new();
        meter.reserve_capacity(&mut values, 1).unwrap();
        assert_eq!((values.capacity(), meter.used()), (4, 16));
        let mut bytes: Vec<u8> = Vec::new();
        meter.reserve_capacity(&mut bytes, 1).unwrap();
        assert_eq!((bytes.capacity(), meter.used()), (8, 24));
    }

    #[test]
    fn capacity_growth_uses_exact_room_when_doubling_would_not_fit() {
        let mut meter = Meter::new(40, MESSAGE);
        let mut values = Vec::new();
        meter.reserve(&mut values, 4).unwrap();
        values.extend([1_u32, 2, 3, 4]);
        // Four old words plus five new words fit; doubling to eight does not.
        meter.reserve_capacity(&mut values, 1).unwrap();
        values.push(5);
        assert_eq!(values, [1, 2, 3, 4, 5]);
        assert_eq!((values.capacity(), meter.used()), (5, 20));
        values.pop();
        meter.reserve_capacity(&mut values, 1).unwrap();
        assert_eq!(meter.used(), 20, "spare capacity was already charged");
    }
}
