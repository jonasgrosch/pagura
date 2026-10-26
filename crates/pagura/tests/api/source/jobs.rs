//! Literal sparse input through the public page/scene/band/PNG composition.

use super::rectangle_source;
use pagura::source::{ByteRange, ByteSource, LoadingLimits, ReadFault, ReadResult, SparseBytes};
use pagura::{
    BandOutput, Error, OpenJob, OpenOptions, Opening, OutputLimits, RenderContext, RenderJob,
    RenderJobLimits, RenderLoading, RenderOptions, Validation, png,
};
use std::sync::Arc;

fn context(source: &Arc<SparseBytes>) -> ReadResult<Arc<RenderContext>> {
    let mut job = OpenJob::new(
        source.clone(),
        &OpenOptions::default(),
        LoadingLimits::default(),
        Validation::RequestedOperation,
    )?;
    for _ in 0..16 {
        match job.advance()? {
            Opening::Yielded => {}
            Opening::Ready(document) => return Ok(Arc::new(RenderContext::new(document))),
            _ => return Err(Error::invalid_argument("unexpected test opening state").into()),
        }
    }
    Err(Error::limit("test opening step bound exceeded").into())
}

fn render(context: &Arc<RenderContext>, limits: RenderJobLimits) -> ReadResult<RenderJob> {
    RenderJob::new(
        context.clone(),
        0,
        RenderOptions::from_dpi(72.0),
        None,
        OutputLimits {
            slots: 1,
            ..OutputLimits::default()
        },
        limits,
    )
}

fn advance(job: &mut RenderJob) -> ReadResult<RenderLoading> {
    for _ in 0..32 {
        match job.advance()? {
            RenderLoading::Yielded => {}
            result => return Ok(result),
        }
    }
    Err(Error::limit("test rendering step bound exceeded").into())
}

#[test]
fn sparse_render_job_delivers_exact_bands_and_png_without_materialization() {
    fn send<T: Send>() {}
    send::<RenderJob>();
    let source = rectangle_source().unwrap();
    let context = context(&source).unwrap();
    let mut job = render(&context, RenderJobLimits::default()).unwrap();
    let Err(ReadFault::Unavailable(first)) = advance(&mut job) else {
        panic!("content object wait")
    };
    assert_eq!(first.ranges(), &[ByteRange::new(1500, 1756).unwrap()]);
    assert_eq!(job.scene_attempts(), 1);
    for _ in 0..3 {
        assert_eq!(
            job.advance().unwrap_err(),
            ReadFault::Unavailable(first.clone())
        );
        assert_eq!(job.scene_attempts(), 1);
    }
    source
        .supply(
            source.identity(),
            1500,
            b"4 0 obj << /Length 13 >> stream\n",
        )
        .unwrap();
    source
        .supply(source.identity(), 1545, b"\nendstream\nendobj\n")
        .unwrap();
    let Err(ReadFault::Unavailable(payload)) = advance(&mut job) else {
        panic!("content payload wait")
    };
    assert_eq!(payload.ranges(), &[ByteRange::new(1532, 1545).unwrap()]);
    assert_eq!(job.scene_attempts(), 2);
    source
        .supply(source.identity(), 1532, b"0 0 8 8 re f\n")
        .unwrap();
    let RenderLoading::Ready(mut bands) = advance(&mut job).unwrap() else {
        panic!("prepared band session")
    };
    assert_eq!(job.scene_attempts(), 3);
    assert!(matches!(job.advance().unwrap(), RenderLoading::Complete));
    assert!(!job.cancel());
    let mut second = render(&context, RenderJobLimits::default()).unwrap();
    let RenderLoading::Ready(png_render) = advance(&mut second).unwrap() else {
        panic!("prepared PNG session")
    };
    assert_eq!(second.scene_attempts(), 1);
    assert!(matches!(
        context.document().materialize(),
        Err(ReadFault::Unavailable(_))
    ));
    assert!(matches!(
        source.read_exact(ByteRange::new(4000, 4001).unwrap()),
        Err(ReadFault::Unavailable(_))
    ));
    assert_eq!(Arc::strong_count(&context), 1);
    source.close();
    drop(context);
    assert!(matches!(job.advance().unwrap(), RenderLoading::Complete));
    let expected = [0, 0, 0, 255].repeat(64);
    let BandOutput::Band(band) = bands.advance() else {
        panic!("one final band")
    };
    assert_eq!(band.rows(), 0..8);
    assert_eq!((band.region().width(), band.region().height()), (8, 8));
    assert_eq!(band.premultiplied_rgba(), expected);
    drop(band);
    assert!(matches!(bands.advance(), BandOutput::Complete));
    let encoder = png::RenderEncoder::new(
        png_render,
        png::Level::Default,
        png::EncodingLimits::default(),
    )
    .unwrap();
    let encoded = crate::png::collect_rendered(encoder).unwrap();
    assert_eq!(
        encoded,
        png::encode(&expected, 8, 8, &png::Options::default()).unwrap()
    );
    assert_eq!(&encoded[16..29], &[0, 0, 0, 8, 0, 0, 0, 8, 8, 6, 0, 0, 0]);
}

#[test]
fn changed_source_retries_charge_scene_attempts_and_failure_drops_context() {
    let source = rectangle_source().unwrap();
    let context = context(&source).unwrap();
    let mut job = render(&context, RenderJobLimits { scene_attempts: 2 }).unwrap();
    assert!(matches!(advance(&mut job), Err(ReadFault::Unavailable(_))));
    assert_eq!(job.scene_attempts(), 1);
    assert_eq!(Arc::strong_count(&context), 2);
    // An unrelated supplied byte changes availability, not the missing content.
    source.supply(source.identity(), 4000, b"5").unwrap();
    assert!(matches!(advance(&mut job), Err(ReadFault::Unavailable(_))));
    assert_eq!(job.scene_attempts(), 2);
    source.supply(source.identity(), 4001, b" ").unwrap();
    let expected = ReadFault::Invalid(Error::limit("scene attempt budget exceeded"));
    assert_eq!(job.advance().unwrap_err(), expected);
    assert_eq!(job.scene_attempts(), 2);
    assert_eq!(Arc::strong_count(&context), 1);
    assert!(!job.cancel());
    source
        .supply(
            source.identity(),
            1500,
            b"4 0 obj << /Length 13 >> stream\n0 0 8 8 re f\n\nendstream\nendobj\n",
        )
        .unwrap();
    assert_eq!(job.advance().unwrap_err(), expected);
    assert_eq!(context.document().diagnostics(), []);
}

#[test]
fn cancelled_and_closed_render_waits_cannot_deliver_output() {
    let source = rectangle_source().unwrap();
    let context = context(&source).unwrap();
    let mut cancelled = render(&context, RenderJobLimits::default()).unwrap();
    assert!(matches!(
        advance(&mut cancelled),
        Err(ReadFault::Unavailable(_))
    ));
    assert!(cancelled.cancel());
    assert!(!cancelled.cancel());
    assert_eq!(Arc::strong_count(&context), 1);
    let mut closed = render(&context, RenderJobLimits::default()).unwrap();
    assert!(matches!(
        advance(&mut closed),
        Err(ReadFault::Unavailable(_))
    ));
    source.close();
    let expected = ReadFault::Invalid(source.check().unwrap_err());
    for _ in 0..3 {
        assert!(matches!(
            cancelled.advance().unwrap(),
            RenderLoading::Cancelled
        ));
        assert_eq!(closed.advance().unwrap_err(), expected);
    }
    assert_eq!(Arc::strong_count(&context), 1);
    assert!(!closed.cancel());
}
