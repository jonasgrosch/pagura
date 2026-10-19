//! The work and memory budgets of one decode (ADR 0016), shared by the
//! codecs: [`Limits::decode_work`] for work, [`Limits::decode_bytes`] for
//! working memory and [`Limits::decoded_bytes`] for the decoded output.
//!
//! Every allocation is charged before it is made. Output is never refunded.
//! A decoder may return working memory it has dropped with
//! [`Budget::release`], so the memory meter bounds what it holds at once
//! rather than everything it allocated; without releases it bounds the
//! total.
use pagura_core::{Limits, Meter, Result};

/// The three meters of one decode, each with its codec's own message.
#[derive(Debug, Clone)]
pub(crate) struct Budget {
    work: Meter,
    memory: Meter,
    output: Meter,
}

/// The `Limit` messages a codec's budget reports.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Messages {
    pub(crate) work: &'static str,
    pub(crate) memory: &'static str,
    pub(crate) output: &'static str,
}

impl Budget {
    pub(crate) const fn new(limits: &Limits, messages: Messages) -> Self {
        Self {
            work: Meter::new(limits.decode_work, messages.work),
            memory: Meter::new(limits.decode_bytes, messages.memory),
            output: Meter::new(limits.decoded_bytes, messages.output),
        }
    }

    /// Charges `units` of work.
    pub(crate) fn work(&mut self, units: usize) -> Result<()> {
        self.work.charge(units)
    }

    /// Charges working memory for `count` values of `T`, for buffers
    /// allocated later with [`vector`].
    pub(crate) fn memory<T>(&mut self, count: usize) -> Result<()> {
        self.memory.charge_items::<T>(count)
    }

    /// Returns the working memory of `count` values of `T` that the decoder
    /// charged through this budget and has dropped. Releasing more than was
    /// charged saturates at zero.
    pub(crate) const fn release<T>(&mut self, count: usize) {
        self.memory.refund(count.saturating_mul(size_of::<T>()));
    }

    /// Charges output for `count` values of `T`, for an output allocated
    /// later with [`vector`].
    pub(crate) fn output_items<T>(&mut self, count: usize) -> Result<()> {
        self.output.charge_items::<T>(count)
    }

    /// An empty working vector with room for `capacity` values, charged to
    /// the memory budget before it is allocated.
    pub(crate) fn with_capacity<T>(&mut self, capacity: usize) -> Result<Vec<T>> {
        let mut values = Vec::new();
        self.memory.reserve(&mut values, capacity)?;
        Ok(values)
    }

    /// An empty output vector with room for `capacity` values, charged to
    /// the output budget before it is allocated.
    pub(crate) fn output_with_capacity<T>(&mut self, capacity: usize) -> Result<Vec<T>> {
        let mut values = Vec::new();
        self.output.reserve(&mut values, capacity)?;
        Ok(values)
    }

    /// A working vector of `count` copies of `value`. Initializing each
    /// value is charged as work too.
    pub(crate) fn filled<T: Clone>(&mut self, count: usize, value: T) -> Result<Vec<T>> {
        let mut values = Vec::new();
        self.memory.reserve(&mut values, count)?;
        self.work.charge(count)?;
        values.resize(count, value);
        Ok(values)
    }

    /// An output vector of `count` copies of `value`, charged to the output
    /// budget and as work.
    pub(crate) fn output<T: Clone>(&mut self, count: usize, value: T) -> Result<Vec<T>> {
        let mut values = Vec::new();
        self.output.reserve(&mut values, count)?;
        self.work.charge(count)?;
        values.resize(count, value);
        Ok(values)
    }

    /// Appends `value` to a working vector, doubling its capacity and
    /// charging the new capacity when it is full.
    pub(crate) fn push<T>(&mut self, values: &mut Vec<T>, value: T) -> Result<()> {
        if values.len() == values.capacity() {
            let additional = values.len().max(1);
            self.memory.reserve(values, additional)?;
            self.work.charge(additional)?;
        }
        values.push(value);
        Ok(())
    }

    /// Appends `bytes` to a working vector, growing it to the next power of
    /// two and charging the new capacity when it is too small.
    pub(crate) fn append(&mut self, values: &mut Vec<u8>, bytes: &[u8]) -> Result<()> {
        let needed = values.len().saturating_add(bytes.len());
        if needed > values.capacity() {
            let capacity = needed.checked_next_power_of_two().unwrap_or(needed);
            self.memory.charge(capacity - values.capacity())?;
            values
                .try_reserve_exact(capacity - values.len())
                .map_err(|_| pagura_core::Error::limit("image codec allocation failed"))?;
        }
        self.work.charge(bytes.len())?;
        values.extend_from_slice(bytes);
        Ok(())
    }
}

/// A vector of `count` copies of `value` whose memory the caller has
/// charged, allocated without aborting on allocation failure.
pub(crate) fn vector<T: Clone>(count: usize, value: T) -> Result<Vec<T>> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| pagura_core::Error::limit("image codec allocation failed"))?;
    values.resize(count, value);
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::{Budget, Messages};
    use pagura_core::{ErrorKind, Limits};

    const MESSAGES: Messages = Messages {
        work: "test work budget exceeded",
        memory: "test memory budget exceeded",
        output: "test output budget exceeded",
    };

    fn budget(work: usize, memory: usize, output: usize) -> Budget {
        Budget::new(
            &Limits {
                decode_work: work,
                decode_bytes: memory,
                decoded_bytes: output,
                ..Limits::default()
            },
            MESSAGES,
        )
    }

    #[test]
    fn allocations_charge_their_meter_before_allocating() {
        let mut budget = budget(100, 16, 8);
        assert_eq!(budget.filled(4, 0_u32).unwrap(), [0; 4]);
        let error = budget.filled(1, 0_u8).unwrap_err();
        assert_eq!(
            (error.kind, error.message),
            (ErrorKind::Limit, MESSAGES.memory)
        );
        assert_eq!(budget.output(8, 1_u8).unwrap().len(), 8);
        assert_eq!(budget.output(1, 1_u8).unwrap_err().message, MESSAGES.output);
        // Refused allocations charge no work: 4 and 8 units so far.
        budget.work(100 - 4 - 8).unwrap();
        assert_eq!(budget.work(1).unwrap_err().message, MESSAGES.work);
    }

    #[test]
    fn reserved_vectors_charge_their_capacity_without_work() {
        let mut budget = budget(0, 8, 4);
        let values = budget.with_capacity::<u32>(2).unwrap();
        assert_eq!((values.len(), values.capacity() >= 2), (0, true));
        assert_eq!(
            budget.with_capacity::<u8>(1).unwrap_err().message,
            MESSAGES.memory
        );
        let output = budget.output_with_capacity::<u16>(2).unwrap();
        assert!(output.capacity() >= 2, "room for two values");
        assert_eq!(
            budget.output_with_capacity::<u8>(1).unwrap_err().message,
            MESSAGES.output
        );
    }

    #[test]
    fn released_memory_can_be_charged_again() {
        let mut budget = budget(100, 16, 0);
        let values = budget.filled(4, 0_u32).unwrap();
        assert_eq!(budget.filled(1, 0_u8).unwrap_err().message, MESSAGES.memory);
        let count = values.len();
        drop(values);
        budget.release::<u32>(count);
        assert_eq!(budget.filled(16, 0_u8).unwrap().len(), 16);
        budget.release::<u64>(usize::MAX);
        assert_eq!(budget.filled(2, 0_u64).unwrap().len(), 2);
    }

    #[test]
    fn growth_charges_the_new_capacity() {
        let mut budget = budget(100, 7, 0);
        let mut values = Vec::new();
        for value in 0..4_u8 {
            budget.push(&mut values, value).unwrap();
        }
        // Growing to capacities 1, 2 and 4 charged 4 bytes; 8 do not fit.
        assert_eq!(
            budget.push(&mut values, 4).unwrap_err().message,
            MESSAGES.memory
        );
        let mut bytes = Vec::new();
        let mut budget = Budget::new(&Limits::default(), MESSAGES);
        budget.append(&mut bytes, &[1, 2, 3]).unwrap();
        assert_eq!((bytes.as_slice(), bytes.capacity()), (&[1, 2, 3][..], 4));
    }
}
