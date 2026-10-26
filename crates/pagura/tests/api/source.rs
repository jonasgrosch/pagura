use pagura::source::{ByteRange, ByteSource, ReadFault, SourceIdentity, SourceLimits, SparseBytes};

#[test]
fn facade_exposes_one_source_and_lease_contract() {
    let identity = SourceIdentity::new(12, [9; 32]);
    let source = SparseBytes::new(
        identity,
        4,
        &pagura::Limits::default(),
        &SourceLimits::default(),
    )
    .unwrap();
    let Err(ReadFault::Unavailable(missing)) = source.materialize() else {
        panic!("expected explicit availability")
    };
    assert_eq!(missing.ranges(), &[ByteRange::new(0, 4).unwrap()]);
    source.supply(identity, 2, &[0, 255]).unwrap();
    source.supply(identity, 0, &[31, 127]).unwrap();
    assert_eq!(source.materialize().unwrap().bytes(), &[31, 127, 0, 255]);
}
