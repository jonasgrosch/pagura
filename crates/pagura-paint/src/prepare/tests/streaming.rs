//! Consumers and direct collectors share execution, quantization and stops.

use super::invariance::busy;
use crate::format::Format;
use crate::testing::{Build, mark};
use crate::{Completion, Scratch};
use pagura_core::{Limits, Store};
use pagura_interpret::{BlendMode, GroupId, StateId};
use pagura_raster::{DeviceRect, Pixmap, Rgba8};

#[test]
fn reusable_bands_match_collectors_across_regions_heights_backgrounds_and_stores() {
    let b = busy();
    let mut scratch = Scratch::default();
    for capacity in [0, 1 << 20] {
        let store = Store::new(capacity);
        for background in [
            Rgba8::WHITE,
            Rgba8::new(20, 40, 60, 128),
            Rgba8::TRANSPARENT,
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
                    let prepared =
                        crate::prepare(&b.scene, &request, &store, &Limits::default()).unwrap();
                    let mut rendering = prepared.with_scratch(&mut scratch).unwrap();
                    let bytes = usize::try_from(region.width() * region.height()).unwrap() * 4;
                    let mut collected = Vec::with_capacity(bytes + 3);
                    collected.extend_from_slice(&[7, 8, 9]);
                    let collector = collected.as_ptr();
                    rendering.render(&mut collected);
                    assert_eq!(collected.as_ptr(), collector);
                    let mut band = Vec::with_capacity(rendering.band_bytes().unwrap());
                    let pointer = band.as_ptr();
                    let capacity = band.capacity();
                    for _ in 0..2 {
                        band.extend_from_slice(&[1, 2, 3]);
                        let mut streamed = Vec::new();
                        let mut next = region.y0;
                        let completion = rendering
                            .render_bands(
                                &mut band,
                                |_| false,
                                |rows, rgba| {
                                    assert_eq!(rows.start, next);
                                    assert_eq!(rgba.as_ptr(), pointer);
                                    assert_eq!(
                                        rgba.len(),
                                        rows.len() * usize::try_from(region.width()).unwrap() * 4
                                    );
                                    streamed.extend_from_slice(rgba);
                                    next = rows.end;
                                    Ok::<(), ()>(())
                                },
                            )
                            .unwrap();
                        assert_eq!(completion, Completion::Complete);
                        assert_eq!(next, region.y1);
                        assert_eq!(streamed, collected[3..]);
                        assert_eq!(band.capacity(), capacity);
                        assert_eq!(band, []);
                        super::assert_within_charges(&rendering, prepared.charges());
                    }
                }
            }
        }
    }
}

#[test]
fn stops_and_consumer_errors_keep_the_next_render_reproducible() {
    let b = busy();
    let request = b.request(b.page(), 5);
    let prepared = b.prepare(&request, &Limits::default()).unwrap();
    let mut scratch = Scratch::default();
    let mut rendering = prepared.with_scratch(&mut scratch).unwrap();
    let mut band = Vec::with_capacity(rendering.band_bytes().unwrap());
    let stopped = rendering.render_bands(
        &mut band,
        |_| true,
        |_, _| panic!("stop before first band called the consumer"),
    );
    assert_eq!(stopped, Ok::<_, ()>(Completion::Stopped { row: 0 }));
    assert_eq!(band, []);
    let mut expected = Vec::new();
    rendering.render(&mut expected);
    let mut actual = Vec::new();
    let stopped = rendering
        .render_bands(
            &mut band,
            |row| row >= 10,
            |_, rgba| {
                actual.extend_from_slice(rgba);
                Ok::<(), ()>(())
            },
        )
        .unwrap();
    assert_eq!(stopped, Completion::Stopped { row: 10 });
    assert_eq!(actual, expected[..actual.len()]);
    let mut calls = 0;
    let failed = rendering.render_bands(
        &mut band,
        |_| false,
        |rows, _| {
            calls += 1;
            Err(rows.end)
        },
    );
    assert_eq!(failed, Err(5));
    assert_eq!(calls, 1);
    actual.clear();
    rendering
        .render_bands(
            &mut band,
            |_| false,
            |_, rgba| {
                actual.extend_from_slice(rgba);
                Ok::<(), ()>(())
            },
        )
        .unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn opaque_white_outputs_pack_exactly_like_rgb_over_white_in_every_page_format() {
    for format in [Format::N16, Format::F32, Format::F64] {
        let mut b = Build::new(7.0, 70.0);
        let red = b.rgb([0.8, 0.2, 0.1]);
        b.rect(GroupId(0), [0.5, 0.5, 6.5, 69.5], red, mark(StateId(0)));
        if format != Format::N16 {
            let half = b.state(|s| {
                s.alpha = 0.5;
                if format == Format::F64 {
                    s.blend = BlendMode::Multiply;
                }
            });
            b.rect(GroupId(0), [1.0, 1.0, 6.0, 66.0], red, mark(half));
            b.rect(GroupId(0), [2.0, 2.0, 7.0, 68.0], red, mark(half));
        }
        let prepared = b
            .prepare(&b.request(b.page(), 16), &Limits::default())
            .unwrap();
        assert_eq!(prepared.plan.groups[0].format, format);
        let mut scratch = Scratch::default();
        let mut rendering = prepared.with_scratch(&mut scratch).unwrap();
        let mut rgba = Vec::new();
        rendering.render(&mut rgba);
        let pixmap = Pixmap::from_premultiplied(7, 70, rgba, Limits::default()).unwrap();
        let mut band = Vec::with_capacity(rendering.band_bytes().unwrap());
        let mut rgb = Vec::new();
        rendering
            .render_bands(
                &mut band,
                |_| false,
                |_, rgba| {
                    for pixel in rgba.as_chunks::<4>().0 {
                        assert_eq!(pixel[3], 255);
                        rgb.extend_from_slice(&pixel[..3]);
                    }
                    Ok::<(), ()>(())
                },
            )
            .unwrap();
        assert_eq!(rgb, pixmap.to_rgb_over(Rgba8::WHITE));
        let mut session = crate::RenderSession::new(
            std::sync::Arc::new(prepared),
            scratch,
            crate::OutputLimits::default(),
        )
        .unwrap();
        let mut owned = Vec::new();
        loop {
            match session.advance() {
                crate::BandOutput::Band(band) => owned.extend_from_slice(band.premultiplied_rgba()),
                crate::BandOutput::Complete => break,
                other => panic!("unexpected owned output: {other:?}"),
            }
        }
        assert_eq!(owned, pixmap.data());
    }
}
