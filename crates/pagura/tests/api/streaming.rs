//! Band consumers use the facade alone, with engine and sink errors separate.

use pagura::{
    Completion, Document, ErrorKind, Limits, OpenOptions, Recovery, RenderContext, RenderOptions,
    Rgba8, Tile,
};
use pagura_testkit::fixture;
use std::sync::Arc;

fn context(limits: Limits) -> pagura::Result<RenderContext> {
    Ok(RenderContext::new(Arc::new(Document::open(
        fixture("classic", "rectangle.pdf"),
        &OpenOptions {
            limits,
            ..OpenOptions::default()
        },
    )?)))
}

#[test]
fn streamed_regions_equal_collectors_cold_warm_and_after_stops_or_sink_errors() {
    let context = context(Limits::default()).unwrap();
    let scene = context.record(context.document().page(0).unwrap()).unwrap();
    let tile = Some(Tile {
        x: 3,
        y: 17,
        width: 54,
        height: 42,
    });
    for background in [
        Rgba8::WHITE,
        Rgba8::new(20, 40, 60, 128),
        Rgba8::TRANSPARENT,
    ] {
        let options = RenderOptions {
            background,
            ..RenderOptions::default()
        };
        let expected = context.render_scene(&scene, &options, tile).unwrap();
        for _ in 0..2 {
            let mut actual = Vec::new();
            let mut next = 17;
            let completed = context
                .render_bands(
                    &scene,
                    &options,
                    tile,
                    |_| false,
                    |rows, rgba| {
                        assert_eq!(rows.start, next);
                        assert_eq!(rgba.len(), rows.len() * 54 * 4);
                        actual.extend_from_slice(rgba);
                        next = rows.end;
                        Ok::<(), ()>(())
                    },
                )
                .unwrap()
                .unwrap();
            assert_eq!(completed, Completion::Complete);
            assert_eq!(next, 59);
            assert_eq!(actual, expected.data());
        }
        let mut actual = Vec::new();
        let stopped = context
            .render_bands(
                &scene,
                &options,
                tile,
                |row| row >= 32,
                |_, rgba| {
                    actual.extend_from_slice(rgba);
                    Ok::<(), ()>(())
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(stopped, Completion::Stopped { row: 32 });
        assert_eq!(actual, expected.data()[..actual.len()]);
        let mut calls = 0;
        let failed = context
            .render_bands(
                &scene,
                &options,
                tile,
                |_| false,
                |rows, _| {
                    calls += 1;
                    Err(rows.end)
                },
            )
            .unwrap();
        assert_eq!(failed, Err(32));
        assert_eq!(calls, 1);
        assert_eq!(
            context.render_scene(&scene, &options, tile).unwrap(),
            expected
        );
    }
}

#[test]
fn stream_caps_its_output_band_and_collector_caps_its_full_surface() {
    let expected_context = context(Limits::default()).unwrap();
    let scene = expected_context
        .record(expected_context.document().page(0).unwrap())
        .unwrap();
    let options = RenderOptions::default();
    let expected = expected_context
        .render_scene(&scene, &options, None)
        .unwrap();
    let context = context(Limits {
        decoded_bytes: 64 * 32 * 4,
        ..Limits::default()
    })
    .unwrap();
    let scene = context.record(context.document().page(0).unwrap()).unwrap();
    let error = context.render_scene(&scene, &options, None).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
    assert_eq!(error.message, "surface byte budget exceeded");
    let mut actual = Vec::new();
    context
        .render_bands(
            &scene,
            &options,
            None,
            |_| false,
            |_, rgba| {
                actual.extend_from_slice(rgba);
                Ok::<(), ()>(())
            },
        )
        .unwrap()
        .unwrap();
    assert_eq!(actual, expected.data());
}

#[test]
fn region_budgets_and_strict_preparation_fail_before_any_consumer_call() {
    for limits in [
        Limits {
            decoded_bytes: 64 * 32 * 4 - 1,
            ..Limits::default()
        },
        Limits {
            pixels: 64 * 64 - 1,
            ..Limits::default()
        },
        Limits {
            render_work: 0,
            ..Limits::default()
        },
    ] {
        let context = context(limits).unwrap();
        let scene = context.record(context.document().page(0).unwrap()).unwrap();
        let mut calls = 0;
        let error = context
            .render_bands(
                &scene,
                &RenderOptions::default(),
                None,
                |_| false,
                |_, _| {
                    calls += 1;
                    Ok::<(), ()>(())
                },
            )
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit);
        assert_eq!(calls, 0);
    }
    let context = RenderContext::new(Arc::new(
        Document::open(
            fixture("paint", "image-fax-grid-crop.pdf"),
            &OpenOptions {
                recovery: Recovery::Strict,
                ..OpenOptions::default()
            },
        )
        .unwrap(),
    ));
    let scene = context.record(context.document().page(0).unwrap()).unwrap();
    let mut calls = 0;
    let error = context
        .render_bands(
            &scene,
            &RenderOptions::default(),
            None,
            |_| false,
            |_, _| {
                calls += 1;
                Ok::<(), ()>(())
            },
        )
        .unwrap_err();
    assert_eq!(
        error.repair.map(pagura::Repair::as_str),
        Some("image.ccitt-columns")
    );
    assert_eq!(calls, 0);
}
