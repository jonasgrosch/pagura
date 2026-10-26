//! Owned memory reservations outlive the producer that admitted them.

use pagura_core::{ErrorKind, MemoryBudget};
use std::sync::Arc;

#[test]
fn shared_allocation_is_charged_until_the_last_lease_drops() {
    let budget = MemoryBudget::new(64, "test allocation quota");
    let allocation = Arc::new(budget.reserve(17).unwrap());
    let lease = Arc::clone(&allocation);
    assert_eq!(budget.used(), 17);
    drop(allocation);
    assert_eq!(budget.used(), 17);
    drop(lease);
    assert_eq!(budget.used(), 0);
}

#[test]
fn refused_growth_is_atomic_and_capacity_growth_remains_charged() {
    let budget = MemoryBudget::new(64, "test allocation quota");
    let mut charge = budget.reserve(8).unwrap();
    let mut bytes = Vec::<u8>::new();
    charge.reserve(&mut bytes, 16).unwrap();
    assert!(bytes.capacity() >= 16);
    assert_eq!(charge.bytes(), budget.used());
    let before = budget.used();
    assert_eq!(charge.grow(65).unwrap_err().kind, ErrorKind::Limit);
    assert_eq!(budget.used(), before);
    assert_eq!(charge.grow(usize::MAX).unwrap_err().kind, ErrorKind::Limit);
    drop(bytes);
    drop(charge);
    assert_eq!(budget.used(), 0);
}
