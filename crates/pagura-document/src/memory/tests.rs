use crate::{Document, OpenOptions};
use pagura_core::{ErrorKind, Limits, ObjectId};
use pagura_testkit::fixture;

fn open(name: &str, bytes: usize) -> Document {
    crate::tests::document(
        name,
        pagura_core::Recovery::default(),
        Limits {
            document_bytes: bytes,
            ..Limits::default()
        },
    )
}

#[test]
fn object_caches_stop_at_the_aggregate_budget_and_keep_prior_objects() {
    let document = open("cache-large-values", 16 * 1024);
    let mut loaded = 0;
    for number in 5..=20 {
        match document.object(ObjectId::new(number, 0)) {
            Ok(_) => loaded += 1,
            Err(error) => {
                assert_eq!(error.as_error().unwrap().kind, ErrorKind::Limit);
                assert_eq!(
                    error.as_error().unwrap().object,
                    Some(ObjectId::new(number, 0))
                );
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
    // Entries are refused while the cross-reference data is read, before any
    // object table exists to hold them (ADR 0012).
    assert_eq!(error.kind, ErrorKind::Limit);
    assert_eq!(error.message, "cross-reference entry budget exceeded");
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
    assert_eq!(error.as_error().unwrap().kind, ErrorKind::Limit);
    assert_eq!(error.as_error().unwrap().object, Some(ObjectId::new(7, 0)));
    assert_eq!(document.retained.used(), total);
    // The decoded buffer itself must fit before any compressed value is published.
    let stream_object = probe.object(ObjectId::new(5, 0)).unwrap().retained_bytes();
    let document = open(
        "cache-lazy-object-stream",
        initial + stream_object + stream.retained_bytes() - 1,
    );
    let error = document.object(ObjectId::new(6, 0)).unwrap_err();
    assert_eq!(error.as_error().unwrap().kind, ErrorKind::Limit);
    assert_eq!(error.as_error().unwrap().object, Some(ObjectId::new(5, 0)));
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
fn reconstruction_charges_new_compressed_slots_and_releases_bootstrap_caches() {
    let name = "rebuild-compressed-catalog";
    let attempt = |bytes: usize| {
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
    };
    let probe = open(name, usize::MAX);
    let total = probe.retained.used();
    // Opening needs its peak, the largest budget in use while reconstruction
    // read objects through scratch documents. Admission is monotonic in the
    // budget, so a search finds the smallest budget that opens.
    let (mut low, mut high) = (0, usize::try_from(u32::MAX).unwrap());
    while low < high {
        let middle = low + (high - low) / 2;
        if attempt(middle).is_ok() {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    let peak = low;
    let failure = attempt(peak - 1).unwrap_err();
    assert_eq!(failure.kind, ErrorKind::Limit);
    assert!(failure.object.is_some());
    let exact = attempt(peak).unwrap();
    assert_eq!(exact.retained.used(), total);
    // The catalog scan cached every object in a scratch document; only what
    // the opened document itself loaded stays charged.
    assert!(total < peak, "{total} < {peak}");
    let before = exact.retained.used();
    exact.object(ObjectId::new(1, 0)).unwrap();
    assert_eq!(exact.retained.used(), before);
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
    budget
        .charge(usize::MAX, Some(ObjectId::new(1, 0)))
        .unwrap();
    assert_eq!(
        budget
            .charge(1, Some(ObjectId::new(2, 0)))
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
    assert_eq!(budget.used(), usize::MAX);
}
