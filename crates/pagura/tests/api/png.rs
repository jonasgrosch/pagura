//! Public export API contracts; independent CLI pixel decoding lives in Python.

use pagura::{ErrorKind, Limits, Pixmap, Rgba8, png};

#[test]
fn surface_export_uses_straight_alpha_and_preserves_dimensions() {
    let limits = Limits::default();
    let pixels = vec![64, 32, 0, 128, 10, 20, 30, 0, 17, 23, 42, 255];
    let surface = Pixmap::from_premultiplied(3, 1, pixels, limits).unwrap();
    let encoded = png::encode_pixmap(surface, png::Level::Default, &limits).unwrap();
    let expected = png::encode(
        &[128, 64, 0, 128, 0, 0, 0, 0, 17, 23, 42, 255],
        3,
        1,
        &png::Options::default(),
    )
    .unwrap();
    assert_eq!(encoded, expected);
    assert_eq!(&encoded[..8], b"\x89PNG\r\n\x1a\n");
    assert_eq!(&encoded[16..29], &[0, 0, 0, 3, 0, 0, 0, 1, 8, 6, 0, 0, 0]);
}

#[test]
fn public_surface_export_honors_each_caller_budget() {
    for limits in [
        Limits {
            pixels: 1,
            ..Limits::default()
        },
        Limits {
            decoded_bytes: 7,
            ..Limits::default()
        },
        Limits {
            stream_bytes: 64,
            ..Limits::default()
        },
    ] {
        let surface = Pixmap::new(2, 1, Rgba8::TRANSPARENT, 2).unwrap();
        assert_eq!(
            png::encode_pixmap(surface, png::Level::Default, &limits)
                .unwrap_err()
                .kind,
            ErrorKind::Limit
        );
    }
}

pub(super) fn collect_rendered(mut encoder: png::RenderEncoder) -> pagura::Result<Vec<u8>> {
    let mut output = Vec::new();
    loop {
        match encoder.advance()? {
            png::EncodingOutput::Chunk(chunk) => output.extend_from_slice(chunk.bytes()),
            png::EncodingOutput::Yielded => {}
            png::EncodingOutput::Complete => {
                assert_eq!(encoder.encoding_bytes(), 0);
                return Ok(output);
            }
            _ => {
                return Err(pagura::Error::invalid_argument(
                    "unexpected PNG adapter test state",
                ));
            }
        }
    }
}

#[test]
fn rendered_band_png_matches_surface_encoding_for_tiles_and_alpha() {
    use pagura::{OutputLimits, RenderContext, RenderOptions, Tile};
    use pagura_testkit::fixture;
    use std::sync::Arc;
    let document = crate::support::open(fixture("classic", "rectangle.pdf")).unwrap();
    let context = RenderContext::new(Arc::new(document));
    let scene = context.record(context.document().page(0).unwrap()).unwrap();
    for background in [
        Rgba8::WHITE,
        Rgba8::TRANSPARENT,
        Rgba8::new(20, 40, 60, 128),
    ] {
        let options = RenderOptions {
            background,
            ..RenderOptions::default()
        };
        for tile in [
            None,
            Some(Tile {
                x: 3,
                y: 17,
                width: 54,
                height: 42,
            }),
        ] {
            for level in [png::Level::Fast, png::Level::Default] {
                let surface = context.render_scene(&scene, &options, tile).unwrap();
                let expected = png::encode_pixmap(surface, level, &Limits::default()).unwrap();
                let render = context
                    .render_session(
                        &scene,
                        &options,
                        tile,
                        OutputLimits {
                            slots: 1,
                            ..OutputLimits::default()
                        },
                    )
                    .unwrap();
                let encoder =
                    png::RenderEncoder::new(render, level, png::EncodingLimits::default()).unwrap();
                assert_eq!(collect_rendered(encoder).unwrap(), expected);
            }
        }
    }
}

#[test]
fn rendered_png_has_no_full_surface_byte_requirement() {
    use pagura::{Document, OpenOptions, OutputLimits, RenderContext, RenderOptions};
    use pagura_testkit::fixture;
    use std::sync::Arc;
    let context = RenderContext::new(Arc::new(
        Document::open(
            fixture("classic", "rectangle.pdf"),
            &OpenOptions {
                limits: Limits {
                    decoded_bytes: 64 * 32 * 4,
                    ..Limits::default()
                },
                ..OpenOptions::default()
            },
        )
        .unwrap(),
    ));
    let scene = context.record(context.document().page(0).unwrap()).unwrap();
    let options = RenderOptions::default();
    assert_eq!(
        context
            .render_scene(&scene, &options, None)
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
    let render = context
        .render_session(&scene, &options, None, OutputLimits::default())
        .unwrap();
    let output = collect_rendered(
        png::RenderEncoder::new(render, png::Level::Default, png::EncodingLimits::default())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(&output[..8], b"\x89PNG\r\n\x1a\n");
    assert_eq!(&output[16..24], &[0, 0, 0, 64, 0, 0, 0, 64]);
}

#[test]
fn rendered_png_rejects_started_jobs_and_preserves_leases_after_cancellation() {
    use pagura::{OutputLimits, RenderContext, RenderOptions};
    use pagura_testkit::fixture;
    use std::sync::Arc;
    let context = RenderContext::new(Arc::new(
        crate::support::open(fixture("classic", "rectangle.pdf")).unwrap(),
    ));
    let scene = context.record(context.document().page(0).unwrap()).unwrap();
    let mut advanced = context
        .render_session(
            &scene,
            &RenderOptions::default(),
            None,
            OutputLimits::default(),
        )
        .unwrap();
    drop(advanced.advance());
    assert_eq!(
        png::RenderEncoder::new(
            advanced,
            png::Level::Default,
            png::EncodingLimits::default()
        )
        .unwrap_err()
        .kind,
        ErrorKind::InvalidArgument
    );
    let render = context
        .render_session(
            &scene,
            &RenderOptions::default(),
            None,
            OutputLimits::default(),
        )
        .unwrap();
    let mut encoder = png::RenderEncoder::new(
        render,
        png::Level::Default,
        png::EncodingLimits {
            output_slots: 1,
            ..png::EncodingLimits::default()
        },
    )
    .unwrap();
    let png::EncodingOutput::Chunk(header) = encoder.advance().unwrap() else {
        panic!("PNG header")
    };
    assert!(matches!(
        encoder.advance().unwrap(),
        png::EncodingOutput::Backpressured
    ));
    encoder.cancel();
    assert!(matches!(
        encoder.advance().unwrap(),
        png::EncodingOutput::Cancelled
    ));
    assert_eq!(encoder.encoding_bytes(), header.retained_bytes());
    assert_eq!(&header.bytes()[..8], b"\x89PNG\r\n\x1a\n");
    drop(header);
    assert_eq!(encoder.encoding_bytes(), 0);
}
