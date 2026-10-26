//! Persistent cursor and leased storage controls over hand-built scenes.

use super::invariance::busy;
use crate::{BandLease, BandOutput, OutputLimits, RenderSession, Scratch};
use pagura_core::{ErrorKind, Limits, Store};
use pagura_raster::{DeviceRect, Rgba8};
use std::sync::Arc;

fn band(session: &mut RenderSession) -> BandLease {
    let BandOutput::Band(band) = session.advance() else {
        panic!("expected a final band")
    };
    band
}

fn collect(session: &mut RenderSession) -> Vec<u8> {
    let mut pixels = Vec::new();
    loop {
        match session.advance() {
            BandOutput::Band(band) => pixels.extend_from_slice(band.premultiplied_rgba()),
            BandOutput::Complete => return pixels,
            other => panic!("unexpected step: {other:?}"),
        }
    }
}

#[test]
fn paused_owned_sessions_equal_collectors_across_regions_bands_and_stores() {
    let b = busy();
    for capacity in [0, 1 << 20] {
        let store = Store::new(capacity);
        for background in [
            Rgba8::WHITE,
            Rgba8::TRANSPARENT,
            Rgba8::new(20, 40, 60, 128),
        ] {
            for height in [1, 5, 16, 32, 256] {
                for region in [
                    b.page(),
                    DeviceRect {
                        x0: 2,
                        y0: 3,
                        x1: 35,
                        y1: 28,
                    },
                ] {
                    let mut request = b.request(region, height);
                    request.background = background;
                    let prepared = Arc::new(
                        crate::prepare(&b.scene, &request, &store, &Limits::default()).unwrap(),
                    );
                    let mut scratch = Scratch::default();
                    let mut expected = Vec::new();
                    prepared
                        .with_scratch(&mut scratch)
                        .unwrap()
                        .render(&mut expected);
                    let mut session = RenderSession::new(
                        Arc::clone(&prepared),
                        scratch,
                        OutputLimits {
                            slots: 1,
                            ..OutputLimits::default()
                        },
                    )
                    .unwrap();
                    let mut actual = Vec::new();
                    let mut next = region.y0;
                    let mut pointer = None;
                    loop {
                        match session.advance() {
                            BandOutput::Band(band) => {
                                assert_eq!(band.page(), b.page());
                                assert_eq!(band.region(), region);
                                assert_eq!(band.rows().start, next);
                                assert_eq!(
                                    band.premultiplied_rgba().len(),
                                    band.rows().len() * band.stride()
                                );
                                let allocation = band.premultiplied_rgba().as_ptr();
                                assert_eq!(*pointer.get_or_insert(allocation), allocation);
                                next = band.rows().end;
                                actual.extend_from_slice(band.premultiplied_rgba());
                            }
                            BandOutput::Complete => break,
                            other => panic!("unexpected step: {other:?}"),
                        }
                    }
                    assert_eq!(actual, expected);
                    assert_eq!(next, region.y1);
                    assert_eq!(session.charges(), prepared.charges());
                    assert_eq!(session.output_bytes(), 0);
                    assert!(matches!(session.advance(), BandOutput::Complete));
                }
            }
        }
    }
}

#[test]
fn held_leases_stop_execution_and_only_a_released_slot_is_reused() {
    let b = busy();
    let prepared = Arc::new(
        b.prepare(&b.request(b.page(), 5), &Limits::default())
            .unwrap(),
    );
    let mut session =
        RenderSession::new(prepared, Scratch::default(), OutputLimits::default()).unwrap();
    let first = band(&mut session);
    let first_pointer = first.premultiplied_rgba().as_ptr();
    let clone = first.clone();
    let snapshot = clone.premultiplied_rgba().to_vec();
    let second = band(&mut session);
    assert_eq!(second.rows().start, first.rows().end);
    let retained = session.output_bytes();
    for _ in 0..3 {
        assert!(matches!(session.advance(), BandOutput::Backpressured));
    }
    assert_eq!(session.output_bytes(), retained);
    drop(first);
    assert!(matches!(session.advance(), BandOutput::Backpressured));
    assert_eq!(clone.premultiplied_rgba(), snapshot);
    drop(clone);
    let third = band(&mut session);
    assert_eq!(third.premultiplied_rgba().as_ptr(), first_pointer);
    assert_eq!(third.rows().start, second.rows().end);
    let held_bytes = second.retained_bytes() + third.retained_bytes();
    session.cancel();
    session.cancel();
    assert!(matches!(session.advance(), BandOutput::Cancelled));
    assert_eq!(session.output_bytes(), held_bytes);
    drop(second);
    assert_eq!(session.output_bytes(), third.retained_bytes());
    drop(third);
    assert_eq!(session.output_bytes(), 0);
}

#[test]
fn completion_and_cancellation_drop_preparation_but_leave_held_rows_valid() {
    let b = busy();
    let prepared = Arc::new(
        b.prepare(&b.request(b.page(), 256), &Limits::default())
            .unwrap(),
    );
    let weak = Arc::downgrade(&prepared);
    let mut session =
        RenderSession::new(prepared, Scratch::default(), OutputLimits::default()).unwrap();
    let lease = band(&mut session);
    let pixels = lease.premultiplied_rgba().to_vec();
    assert!(weak.upgrade().is_none());
    session.cancel();
    assert!(matches!(session.advance(), BandOutput::Complete));
    assert_eq!(session.output_bytes(), lease.retained_bytes());
    drop(session);
    assert_eq!(lease.premultiplied_rgba(), pixels);
}

#[test]
fn admission_rejects_empty_slots_and_output_quotas_before_execution() {
    let b = busy();
    let prepared = Arc::new(
        b.prepare(&b.request(b.page(), 5), &Limits::default())
            .unwrap(),
    );
    for (limits, kind) in [
        (
            OutputLimits {
                slots: 0,
                bytes: usize::MAX,
            },
            ErrorKind::InvalidArgument,
        ),
        (OutputLimits { slots: 1, bytes: 0 }, ErrorKind::Limit),
        (
            OutputLimits {
                slots: usize::MAX,
                bytes: usize::MAX,
            },
            ErrorKind::Limit,
        ),
    ] {
        let error =
            RenderSession::new(Arc::clone(&prepared), Scratch::default(), limits).unwrap_err();
        assert_eq!(error.kind, kind);
        assert_eq!(Arc::strong_count(&prepared), 1);
    }
    let mut session =
        RenderSession::new(prepared, Scratch::default(), OutputLimits::default()).unwrap();
    session.cancel();
    assert_eq!(session.output_bytes(), 0);
    assert!(matches!(session.advance(), BandOutput::Cancelled));
}

#[test]
fn independent_owned_sessions_share_preparation_across_threads() {
    let b = busy();
    let prepared = Arc::new(
        b.prepare(&b.request(b.page(), 5), &Limits::default())
            .unwrap(),
    );
    let results = std::thread::scope(|scope| {
        let jobs: Vec<_> = (0..2)
            .map(|_| {
                let mut session = RenderSession::new(
                    Arc::clone(&prepared),
                    Scratch::default(),
                    OutputLimits::default(),
                )
                .unwrap();
                scope.spawn(move || collect(&mut session))
            })
            .collect();
        jobs.into_iter()
            .map(|job| job.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(results[0], results[1]);
    assert_eq!(Arc::strong_count(&prepared), 1);
}
