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

#[test]
fn admitted_source_document_uses_the_existing_render_context_and_checks_cached_scenes() {
    use pagura::source::LoadingLimits;
    use pagura::{OpenJob, OpenOptions, Opening, RenderContext};
    use pagura_testkit::fixture;
    use std::sync::Arc;

    let bytes = fixture("classic", "rectangle.pdf");
    let identity = SourceIdentity::new(13, [21; 32]);
    let source = Arc::new(
        SparseBytes::new(
            identity,
            u64::try_from(bytes.len()).unwrap(),
            &pagura::Limits::default(),
            &SourceLimits::default(),
        )
        .unwrap(),
    );
    let mut job = OpenJob::new(
        source.clone(),
        &OpenOptions::default(),
        LoadingLimits::default(),
    )
    .unwrap();
    assert!(matches!(job.advance(), Err(ReadFault::Unavailable(_))));
    source.supply(identity, 0, &bytes).unwrap();
    for _ in 0..3 {
        assert!(matches!(job.advance().unwrap(), Opening::Yielded));
    }
    let Opening::Ready(document) = job.advance().unwrap() else {
        panic!("source document")
    };
    let context = RenderContext::new(document);
    let page = context.document().page(0).unwrap();
    let first = context.scene(page).unwrap();
    assert!(Arc::ptr_eq(&first, &context.scene(page).unwrap()));
    source.close();
    assert_eq!(
        context.scene(page).unwrap_err(),
        source.check().unwrap_err()
    );
    assert!(matches!(job.advance().unwrap(), Opening::Complete));
    drop(first);
    drop(context);
    assert_eq!(source.retained_bytes(), 0);
}
