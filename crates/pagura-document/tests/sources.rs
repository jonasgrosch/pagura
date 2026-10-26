//! Independent byte-store controls; no PDF parser or product writer is an oracle.

use pagura_core::{ErrorKind, Limits};
use pagura_document::source::{
    ByteRange, ByteSource, CompleteBytes, ReadFault, SourceIdentity, SourceLimits, SparseBytes,
};
use std::sync::Arc;

const ID: SourceIdentity = SourceIdentity::new(7, [42; 32]);

fn sparse_source(length: u64, limits: SourceLimits) -> pagura_core::Result<SparseBytes> {
    SparseBytes::new(ID, length, &Limits::default(), &limits)
}

#[test]
fn complete_source_leases_original_allocation_and_checks_bounds() {
    let bytes: Arc<[u8]> = Arc::from(&b"independent bytes"[..]);
    let source = CompleteBytes::new(
        ID,
        Arc::clone(&bytes),
        &Limits::default(),
        &SourceLimits::default(),
    )
    .unwrap();
    let lease = source.materialize().unwrap();
    assert_eq!(lease.bytes(), b"independent bytes");
    assert_eq!(lease.bytes().as_ptr(), bytes.as_ptr());
    assert_eq!(lease.range(), ByteRange::new(0, 17).unwrap());
    assert_eq!(source.identity(), ID);
    assert_eq!(source.generation(), 0);
    let part = source.read_exact(ByteRange::new(12, 17).unwrap()).unwrap();
    assert_eq!(part.bytes(), b"bytes");
    assert_eq!(part.bytes().as_ptr(), bytes[12..].as_ptr());
    assert_eq!(part.range(), ByteRange::new(12, 17).unwrap());
    let empty = source.read_exact(ByteRange::new(17, 17).unwrap()).unwrap();
    assert!(empty.bytes().is_empty());
    assert_eq!(empty.range(), ByteRange::new(17, 17).unwrap());
    assert!(
        matches!(source.read_exact(ByteRange::new(16, 18).unwrap()), Err(ReadFault::Invalid(error)) if error.kind == ErrorKind::InvalidArgument)
    );
    drop(source);
    assert_eq!(lease.bytes(), b"independent bytes");
}

#[test]
fn missing_frontiers_are_exact_sorted_and_versioned() {
    let source = sparse_source(10, SourceLimits::default()).unwrap();
    source.supply(ID, 6, b"gh").unwrap();
    source.supply(ID, 2, b"cd").unwrap();
    let Err(ReadFault::Unavailable(missing)) = source.materialize() else {
        panic!("expected missing ranges")
    };
    assert_eq!(missing.identity(), ID);
    assert_eq!(missing.generation(), 2);
    assert_eq!(
        missing.ranges(),
        &[
            ByteRange::new(0, 2).unwrap(),
            ByteRange::new(4, 6).unwrap(),
            ByteRange::new(8, 10).unwrap()
        ]
    );
    drop(missing);
    let supplied = source.supply(ID, 1, b"bcdefghi").unwrap();
    assert_eq!((supplied.added_bytes(), supplied.generation()), (4, 3));
    source.supply(ID, 0, b"a").unwrap();
    source.supply(ID, 9, b"j").unwrap();
    assert_eq!(source.materialize().unwrap().bytes(), b"abcdefghij");
}

#[test]
fn duplicate_supply_preserves_generation_storage_and_leases() {
    let source = sparse_source(8, SourceLimits::default()).unwrap();
    source.supply(ID, 0, b"abcd").unwrap();
    source.supply(ID, 4, b"efgh").unwrap();
    let before = source.retained_bytes();
    let duplicate = source.supply(ID, 1, b"bcdefg").unwrap();
    assert_eq!((duplicate.added_bytes(), duplicate.generation()), (0, 2));
    assert_eq!(source.retained_bytes(), before);
    let lease = source.read_exact(ByteRange::new(1, 3).unwrap()).unwrap();
    let clone = lease.clone();
    assert_eq!(source.retained_bytes(), before);
    source.close();
    assert_eq!(lease.bytes(), b"bc");
    let held = source.retained_bytes();
    assert!(held > 0 && held < before);
    drop(lease);
    assert_eq!(source.retained_bytes(), held);
    drop(clone);
    assert_eq!(source.retained_bytes(), 0);
    assert!(
        matches!(source.materialize(), Err(ReadFault::Invalid(error)) if error.kind == ErrorKind::InvalidArgument)
    );
}

#[test]
fn conflict_rejects_entire_supply_and_permanently_invalidates_source() {
    let source = sparse_source(8, SourceLimits::default()).unwrap();
    source.supply(ID, 2, b"cd").unwrap();
    source.supply(ID, 6, b"gh").unwrap();
    let lease = source.read_exact(ByteRange::new(2, 4).unwrap()).unwrap();
    let before = source.retained_bytes();
    assert_eq!(
        source.supply(ID, 0, b"abcdefgX").unwrap_err().kind,
        ErrorKind::InvalidArgument
    );
    assert_eq!(source.generation(), 2);
    assert_eq!(source.retained_bytes(), before);
    assert_eq!(lease.bytes(), b"cd");
    assert!(matches!(
        source.read_exact(ByteRange::new(2, 4).unwrap()),
        Err(ReadFault::Invalid(_))
    ));
    assert!(source.supply(ID, 0, b"abcdefgh").is_err());
}

#[test]
fn stale_identity_and_invalid_extents_do_not_poison_current_source() {
    let source = sparse_source(4, SourceLimits::default()).unwrap();
    let stale = SourceIdentity::new(ID.registration(), [43; 32]);
    assert_eq!(
        source.supply(stale, 0, b"abcd").unwrap_err().kind,
        ErrorKind::InvalidArgument
    );
    assert_eq!(
        source.supply(ID, 3, b"xx").unwrap_err().kind,
        ErrorKind::InvalidArgument
    );
    assert_eq!(
        source.supply(ID, u64::MAX, b"xx").unwrap_err().kind,
        ErrorKind::Limit
    );
    assert_eq!(source.generation(), 0);
    source.supply(ID, 0, b"abcd").unwrap();
    assert_eq!(source.materialize().unwrap().bytes(), b"abcd");
    assert!(ByteRange::new(2, 1).is_err());
}

#[test]
fn materialization_is_explicitly_charged_and_assembly_bounded() {
    let source = sparse_source(8, SourceLimits::default()).unwrap();
    source.supply(ID, 0, b"abcd").unwrap();
    source.supply(ID, 4, b"efgh").unwrap();
    let before = source.retained_bytes();
    let joined = source.materialize().unwrap();
    assert_eq!(joined.bytes(), b"abcdefgh");
    assert!(source.retained_bytes() > before);
    source.close();
    assert!(source.retained_bytes() > 0);
    drop(joined);
    assert_eq!(source.retained_bytes(), 0);

    let bounded = sparse_source(
        8,
        SourceLimits {
            assembly_bytes: 4,
            ..SourceLimits::default()
        },
    )
    .unwrap();
    bounded.supply(ID, 0, b"abcd").unwrap();
    bounded.supply(ID, 4, b"efgh").unwrap();
    assert!(
        matches!(bounded.materialize(), Err(ReadFault::Invalid(error)) if error.kind == ErrorKind::Limit)
    );
    assert_eq!(
        bounded
            .read_exact(ByteRange::new(1, 5).unwrap())
            .unwrap()
            .bytes(),
        b"bcde"
    );
}

#[test]
fn source_and_frontier_limits_release_failed_reservations() {
    let source = sparse_source(
        8,
        SourceLimits {
            retained_bytes: 1,
            ..SourceLimits::default()
        },
    )
    .unwrap();
    assert_eq!(
        source.supply(ID, 0, b"abcdefgh").unwrap_err().kind,
        ErrorKind::Limit
    );
    assert_eq!((source.generation(), source.retained_bytes()), (0, 0));

    let bounded = sparse_source(
        8,
        SourceLimits {
            chunks: 1,
            missing_ranges: 1,
            ..SourceLimits::default()
        },
    )
    .unwrap();
    bounded.supply(ID, 2, b"cd").unwrap();
    let before = bounded.retained_bytes();
    assert_eq!(
        bounded.supply(ID, 6, b"gh").unwrap_err().kind,
        ErrorKind::Limit
    );
    assert_eq!(
        (bounded.generation(), bounded.retained_bytes()),
        (1, before)
    );
    assert!(
        matches!(bounded.materialize(), Err(ReadFault::Invalid(error)) if error.kind == ErrorKind::Limit)
    );
    assert_eq!(bounded.retained_bytes(), before);
}

#[test]
fn missing_frontier_lease_stays_charged_after_close() {
    let source = sparse_source(8, SourceLimits::default()).unwrap();
    let Err(ReadFault::Unavailable(missing)) = source.materialize() else {
        panic!("expected missing ranges")
    };
    let copy = missing.clone();
    let held = source.retained_bytes();
    source.close();
    assert!(held > 0);
    assert_eq!(source.retained_bytes(), held);
    drop(missing);
    assert_eq!(source.retained_bytes(), held);
    drop(copy);
    assert_eq!(source.retained_bytes(), 0);
}

#[test]
fn empty_ranges_and_cumulative_work_keep_distinct_meanings() {
    let empty = sparse_source(0, SourceLimits::default()).unwrap();
    assert!(empty.is_empty());
    assert_eq!(empty.materialize().unwrap().bytes(), b"");
    assert_eq!(empty.supply(ID, 0, b"").unwrap().added_bytes(), 0);
    assert_eq!(empty.retained_bytes(), 0);

    let bounded = sparse_source(
        1,
        SourceLimits {
            work: 2,
            ..SourceLimits::default()
        },
    )
    .unwrap();
    bounded.supply(ID, 0, b"x").unwrap();
    assert_eq!(
        bounded.supply(ID, 0, b"x").unwrap_err().kind,
        ErrorKind::Limit
    );
    assert_eq!(bounded.generation(), 1);
}

#[test]
fn arrival_order_does_not_change_independent_bytes() {
    for order in [
        [0_u64, 2, 4, 6, 8],
        [8, 6, 4, 2, 0],
        [4, 0, 8, 2, 6],
        [2, 8, 6, 0, 4],
    ] {
        let source = sparse_source(10, SourceLimits::default()).unwrap();
        let expected = b"0123456789";
        for offset in order {
            let start = usize::try_from(offset).unwrap();
            source
                .supply(ID, offset, &expected[start..start + 2])
                .unwrap();
        }
        assert_eq!(source.materialize().unwrap().bytes(), expected);
    }
}
