//! Source allocation shapes over the shared core reservation owner.

use pagura_core::Result;
pub(super) use pagura_core::{MemoryBudget as Budget, Reservation};
use std::sync::Arc;

pub(super) const EXCEEDED: &str = "source retained-memory budget exceeded";
const ARC_COUNTS: usize = 2 * size_of::<usize>();

pub(super) fn vector<T>(budget: &Arc<Budget>, count: usize) -> Result<(Vec<T>, Reservation)> {
    let header = size_of::<Retained<Vec<T>>>().max(size_of::<Retained<Storage>>());
    let mut reservation = budget.reserve(header + ARC_COUNTS)?;
    let mut values = Vec::new();
    reservation.reserve(&mut values, count)?;
    Ok((values, reservation))
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
