//! Reservations shared by source allocations and their leases.

use pagura_core::{Meter, Result};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

const EXCEEDED: &str = "source retained-memory budget exceeded";
const ARC_COUNTS: usize = 2 * size_of::<usize>();

#[derive(Debug)]
pub(super) struct Budget(Mutex<Meter>);

impl Budget {
    pub(super) fn new(limit: usize) -> Arc<Self> {
        Arc::new(Self(Mutex::new(Meter::new(limit, EXCEEDED))))
    }

    fn meter(&self) -> MutexGuard<'_, Meter> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(super) fn used(&self) -> usize {
        self.meter().used()
    }

    pub(super) fn reserve(self: &Arc<Self>, bytes: usize) -> Result<Reservation> {
        self.meter().charge(bytes)?;
        Ok(Reservation {
            budget: Arc::clone(self),
            bytes,
        })
    }

    pub(super) fn vector<T>(self: &Arc<Self>, count: usize) -> Result<(Vec<T>, Reservation)> {
        let header = size_of::<Retained<Vec<T>>>().max(size_of::<Retained<Storage>>());
        let mut reservation = self.reserve(header + ARC_COUNTS)?;
        let mut values = Vec::new();
        reservation.reserve(&mut values, count)?;
        Ok((values, reservation))
    }
}

#[derive(Debug)]
pub(super) struct Reservation {
    budget: Arc<Budget>,
    bytes: usize,
}

impl Reservation {
    pub(super) fn reserve<T>(&mut self, values: &mut Vec<T>, additional: usize) -> Result<()> {
        let mut meter = self.budget.meter();
        let before = meter.used();
        let result = meter.reserve_capacity(values, additional);
        self.bytes = self
            .bytes
            .saturating_add(meter.used().saturating_sub(before));
        result
    }

    pub(super) fn grow(&mut self, bytes: usize) -> Result<()> {
        self.budget.meter().charge(bytes)?;
        // The checked aggregate contains both the old and new reservation.
        self.bytes = self.bytes.saturating_add(bytes);
        Ok(())
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.budget.meter().refund(self.bytes);
    }
}

#[derive(Debug)]
pub(super) struct Retained<T> {
    pub(super) value: T,
    _reservation: Reservation,
}

impl<T> Retained<T> {
    pub(super) fn new(value: T, reservation: Reservation) -> Arc<Self> {
        Arc::new(Self {
            value,
            _reservation: reservation,
        })
    }
}

#[derive(Debug)]
pub(super) enum Storage {
    Shared(Arc<[u8]>),
    Owned(Vec<u8>),
}

impl Storage {
    pub(super) fn bytes(&self) -> &[u8] {
        match self {
            Self::Shared(bytes) => bytes,
            Self::Owned(bytes) => bytes,
        }
    }
}
