mod diagnostics;

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
        pagura::Validation::CompleteDocument,
    )
    .unwrap();
    assert!(matches!(job.advance(), Err(ReadFault::Unavailable(_))));
    source.supply(identity, 0, &bytes).unwrap();
    for _ in 0..4 {
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
        source.check().unwrap_err().into()
    );
    assert!(matches!(job.advance().unwrap(), Opening::Complete));
    drop(first);
    drop(context);
    assert_eq!(source.retained_bytes(), 0);
}

fn rectangle_source() -> pagura::Result<std::sync::Arc<SparseBytes>> {
    let identity = SourceIdentity::new(14, [27; 32]);
    let source = std::sync::Arc::new(SparseBytes::new(
        identity,
        8192,
        &pagura::Limits::default(),
        &SourceLimits::default(),
    )?);
    for (offset, part) in [
        (0, b"%PDF-1.7\n".as_slice()),
        (100, b"1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n".as_slice()),
        (600, b"2 0 obj << /Type /Pages /Count 1 /Kids [3 0 R] /MediaBox [0 0 8 8] >> endobj\n".as_slice()),
        (900, b"3 0 obj << /Type /Page /Parent 2 0 R /Contents 4 0 R >> endobj\n".as_slice()),
        (6500, b"xref\n0 6\n0 65535 f\n100 0 n\n600 0 n\n900 0 n\n1500 0 n\n4000 0 n\ntrailer << /Size 6 /Root 1 0 R >>\n".as_slice()),
    ] {
        source.supply(identity, offset, part)?;
    }
    let mut tail = vec![b' '; 1024];
    tail[932..953].copy_from_slice(b"startxref\n6500\n%%EOF\n");
    source.supply(identity, 7168, &tail)?;
    Ok(source)
}

#[test]
fn sparse_scene_retries_publish_once_and_render_exact_pixels_without_unrelated_bytes() {
    use pagura::source::LoadingLimits;
    use pagura::{
        BandOutput, OpenJob, OpenOptions, Opening, OutputLimits, RenderContext, RenderOptions,
        Validation,
    };
    use std::sync::Arc;

    let source = rectangle_source().unwrap();
    let identity = source.identity();
    let mut job = OpenJob::new(
        source.clone(),
        &OpenOptions::default(),
        LoadingLimits::default(),
        Validation::RequestedOperation,
    )
    .unwrap();
    let mut ready = None;
    for _ in 0..16 {
        match job.advance().unwrap() {
            Opening::Yielded => {}
            Opening::Ready(document) => {
                ready = Some(document);
                break;
            }
            other => panic!("unexpected opening state {other:?}"),
        }
    }
    let context = RenderContext::new(ready.unwrap());
    let page = context.document().page(0).unwrap();
    for _ in 0..3 {
        let Err(ReadFault::Unavailable(missing)) = context.scene(page) else {
            panic!("content object wait")
        };
        assert_eq!(missing.ranges(), &[ByteRange::new(1500, 1756).unwrap()]);
        assert_eq!(context.document().diagnostics(), []);
    }
    source
        .supply(identity, 1500, b"4 0 obj << /Length 13 >> stream\n")
        .unwrap();
    source
        .supply(identity, 1545, b"\nendstream\nendobj\n")
        .unwrap();
    for _ in 0..3 {
        let Err(ReadFault::Unavailable(missing)) = context.scene(page) else {
            panic!("content payload wait")
        };
        assert_eq!(missing.ranges(), &[ByteRange::new(1532, 1545).unwrap()]);
        assert_eq!(context.document().diagnostics(), []);
    }
    source.supply(identity, 1532, b"0 0 8 8 re f\n").unwrap();
    let scene = context.scene(page).unwrap();
    assert!(Arc::ptr_eq(&scene, &context.scene(page).unwrap()));
    let options = RenderOptions::from_dpi(72.0);
    let pixels = context.render_page(page, &options).unwrap();
    assert_eq!((pixels.width(), pixels.height()), (8, 8));
    assert_eq!(pixels.data(), [0, 0, 0, 255].repeat(64));
    assert!(matches!(
        source.read_exact(ByteRange::new(4000, 4001).unwrap()),
        Err(ReadFault::Unavailable(_))
    ));
    assert!(matches!(
        context.document().materialize(),
        Err(ReadFault::Unavailable(_))
    ));
    let mut output = context
        .render_session(&scene, &options, None, OutputLimits::default())
        .unwrap();
    let BandOutput::Band(band) = output.advance() else {
        panic!("prepared band")
    };
    assert_eq!(band.rows(), 0..8);
    assert_eq!(band.premultiplied_rgba(), pixels.data());
    drop(band);
    assert!(matches!(output.advance(), BandOutput::Complete));
    source.close();
    assert!(context.scene(page).is_err());
}
