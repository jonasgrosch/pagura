//! Aggregate retained-cache accounting (ADR 0012).

use pagura_core::{Error, ObjectId, Result};
use std::sync::{Mutex, PoisonError};

pub(crate) const fn slot_bytes() -> usize {
    size_of::<(ObjectId, crate::Slot)>().saturating_add(64)
}

pub(crate) const fn stream_slot_bytes() -> usize {
    size_of::<(
        u32,
        std::sync::OnceLock<Result<crate::object_stream::Contents>>,
    )>()
    .saturating_add(64)
}

#[derive(Debug)]
pub(crate) struct Budget {
    limit: usize,
    used: Mutex<usize>,
}

impl Budget {
    pub(crate) const fn new(limit: usize) -> Self {
        Self {
            limit,
            used: Mutex::new(0),
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
        *used = next;
        Ok(())
    }

    pub(crate) fn release(&self, bytes: usize) {
        let mut used = self.used.lock().unwrap_or_else(PoisonError::into_inner);
        *used = used.saturating_sub(bytes);
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
mod tests {
    use crate::{Document, OpenOptions};
    use pagura_core::{ErrorKind, Limits, ObjectId};
    use pagura_testkit::fixture;

    fn open(name: &str, bytes: usize) -> Document {
        Document::open(
            fixture("document", &format!("{name}.pdf")),
            &OpenOptions {
                limits: Limits {
                    document_bytes: bytes,
                    ..Limits::default()
                },
                ..OpenOptions::default()
            },
        )
        .unwrap()
    }

    #[test]
    fn object_caches_stop_at_the_aggregate_budget_and_keep_prior_objects() {
        let document = open("cache-large-values", 16 * 1024);
        let mut loaded = 0;
        for number in 5..=20 {
            match document.object(ObjectId::new(number, 0)) {
                Ok(_) => loaded += 1,
                Err(error) => {
                    assert_eq!(error.kind, ErrorKind::Limit);
                    assert_eq!(error.object, Some(ObjectId::new(number, 0)));
                    let retained = document.retained.used();
                    assert_eq!(
                        document.object(ObjectId::new(number, 0)).unwrap_err(),
                        error
                    );
                    assert_eq!(document.retained.used(), retained);
                    break;
                }
            }
        }
        assert!((1..16).contains(&loaded));
        assert!(document.object(ObjectId::new(5, 0)).is_ok());
        assert!(document.retained.used() <= 16 * 1024);
        let error = Document::open(
            fixture("document", "xref-stream.pdf"),
            &OpenOptions {
                limits: Limits {
                    document_bytes: 0,
                    ..Limits::default()
                },
                ..OpenOptions::default()
            },
        )
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit);
        assert_eq!(error.object, Some(ObjectId::new(1, 0)));
    }

    #[test]
    fn decoded_object_streams_and_concurrent_object_access_are_charged_once() {
        let probe = open("cache-lazy-object-stream", usize::MAX);
        let initial = probe.retained.used();
        probe.object(ObjectId::new(6, 0)).unwrap();
        let total = probe.retained.used();
        let stream = probe
            .object_streams
            .get(&5)
            .unwrap()
            .get()
            .unwrap()
            .as_ref()
            .unwrap();
        let expected = probe.object(ObjectId::new(5, 0)).unwrap().retained_bytes()
            + stream.retained_bytes()
            + probe.object(ObjectId::new(6, 0)).unwrap().retained_bytes();
        assert_eq!(total - initial, expected);
        let document = open("cache-lazy-object-stream", total);
        let barrier = std::sync::Barrier::new(8);
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        document.object(ObjectId::new(6, 0)).unwrap()
                    })
                })
                .collect();
            for handle in handles {
                assert_eq!(handle.join().unwrap().id(), ObjectId::new(6, 0));
            }
        });
        assert_eq!(document.retained.used(), total);
        let error = document.object(ObjectId::new(7, 0)).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit);
        assert_eq!(error.object, Some(ObjectId::new(7, 0)));
        assert_eq!(document.retained.used(), total);
        // The decoded buffer itself must fit before any compressed value is published.
        let stream_object = probe.object(ObjectId::new(5, 0)).unwrap().retained_bytes();
        let document = open(
            "cache-lazy-object-stream",
            initial + stream_object + stream.retained_bytes() - 1,
        );
        let error = document.object(ObjectId::new(6, 0)).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit);
        assert_eq!(error.object, Some(ObjectId::new(5, 0)));
        assert!(
            document
                .object_streams
                .get(&5)
                .unwrap()
                .get()
                .unwrap()
                .is_err()
        );
        assert_eq!(document.retained.used(), initial + stream_object);
    }

    #[test]
    fn reconstruction_charges_new_compressed_slots_and_reuses_stream_storage() {
        let probe = open("rebuild-compressed-catalog", usize::MAX);
        let total = probe.retained.used();
        let exact = open("rebuild-compressed-catalog", total);
        assert_eq!(exact.retained.used(), total);
        let before = exact.retained.used();
        exact.object(ObjectId::new(1, 0)).unwrap();
        assert_eq!(exact.retained.used(), before);
        let failure = Document::open(
            fixture("document", "rebuild-compressed-catalog.pdf"),
            &OpenOptions {
                limits: Limits {
                    document_bytes: total - 1,
                    ..Limits::default()
                },
                ..OpenOptions::default()
            },
        )
        .unwrap_err();
        assert_eq!(failure.kind, ErrorKind::Limit);
        assert!(failure.object.is_some());
        assert!(total >= exact.slots.len() * super::slot_bytes());
    }

    #[test]
    fn reservations_rollback_and_overflow_is_a_limit_error() {
        let budget = super::Budget::new(100);
        {
            let mut reservation = budget.reservation();
            reservation.charge(80, ObjectId::new(1, 0)).unwrap();
            assert!(reservation.charge(30, ObjectId::new(2, 0)).is_err());
            assert_eq!(budget.used(), 80);
        }
        assert_eq!(budget.used(), 0);
        let budget = super::Budget::new(usize::MAX);
        budget.charge(usize::MAX, ObjectId::new(1, 0)).unwrap();
        assert_eq!(
            budget.charge(1, ObjectId::new(2, 0)).unwrap_err().kind,
            ErrorKind::Limit
        );
        assert_eq!(budget.used(), usize::MAX);
    }
}
