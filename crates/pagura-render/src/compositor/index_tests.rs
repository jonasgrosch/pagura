//! Integration checks for lazy hull reuse; command traversal remains authoritative.
use super::*;
use crate::display::{GlyphHulls, GlyphRun};
use crate::transparency::{Attributes, ViewFit};
use pagura_core::{ObjectId, Position, Rect};

fn position(offset: usize) -> Position {
    Position::Stream {
        object: ObjectId::new(17, 0),
        offset,
    }
}

fn run(origin: Point, sources: usize) -> GlyphRun {
    let paint = PathCommand {
        path: Arc::default(),
        mark: Mark::Fill(FillRule::NonZero),
        clip: None,
        color: crate::color::Color::Solid {
            space: crate::color::Space::Rgb,
            components: vec![0.2, 0.5, 0.8],
        },
        position: position(10),
        transfer: None,
        composite: Composite::opaque(),
    };
    let mut run = GlyphRun::new(paint);
    let mut hulls = GlyphHulls::default();
    for source in 0..sources {
        let mut outline = PathBuilder::new();
        outline.rect(0.0, 0.0, if source == 0 { 0.75 } else { 1.25 }, 1.5);
        let outline = Arc::new(outline.finish());
        for shift in [0.0, 0.5] {
            run.push(
                Arc::clone(&outline),
                Transform::translate(origin.x + shift, origin.y),
                position(20 + run.glyphs.len()),
                &mut hulls,
            )
            .unwrap();
        }
    }
    run
}

fn list(commands: Vec<Command>) -> DisplayList {
    DisplayList {
        commands,
        crop: Rect::from_corners(0.0, 0.0, 8.0, 8.0).unwrap(),
        rotation: pagura_document::Rotation::None,
        user_unit: 1.0,
        limits: Limits::default(),
        group: Attributes::default(),
        print_intent: false,
        separations: std::sync::OnceLock::new(),
    }
}

fn context(list: &DisplayList, device: Transform, enabled: bool) -> Context {
    let options = RenderOptions::default();
    let separations = Arc::new(Separations::default());
    let plan = plan::page(list, &separations).unwrap();
    let mut context = Context::new(list, &options, &plan, separations).unwrap();
    context.scene = index::SceneIndex::new(enabled);
    context.device = device;
    context.page_device = device;
    context.bounds = regions::Prepared::new(device, list.limits.decoded_bytes);
    context
}

fn tile(x: u32, y: u32, width: u32, height: u32) -> Tile {
    Tile {
        x,
        y,
        width,
        height,
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    result: Result<()>,
    work: usize,
    pixels: Vec<[u64; 7]>,
    bytes: Vec<u8>,
}

fn paint(context: &mut Context, list: &DisplayList, tile: Tile, format: plan::Format) -> Snapshot {
    let count = usize::try_from(u64::from(tile.width) * u64::from(tile.height)).unwrap();
    let mut surface = if format == plan::Format::F64 {
        Surface::new(tile, vec![Pixel::source([0.3, 0.6, 0.9, 0.0], 1.0); count])
    } else if format == plan::Format::U8 {
        Surface::with_buffer(
            tile,
            narrow::Buffer::U8(
                narrow::Bytes::new(
                    (
                        usize::try_from(tile.width).unwrap(),
                        usize::try_from(tile.height).unwrap(),
                    ),
                    [77, 153, 230, 255],
                    Vec::new(),
                )
                .unwrap(),
            ),
        )
    } else {
        Surface::with_buffer(
            tile,
            narrow::Buffer::new(format, count, [77, 153, 230, 255]).unwrap(),
        )
    };
    context.clip.clear();
    context.discard_mask();
    let result = context.commands(
        &list.commands,
        &mut surface,
        Backdrop::Transparent,
        Model::Rgb,
        false,
        0,
    );
    let pixels = surface
        .pixels()
        .iter()
        .map(|pixel| {
            [
                pixel.color[0],
                pixel.color[1],
                pixel.color[2],
                pixel.color[3],
                pixel.alpha,
                pixel.group,
                pixel.shape,
            ]
            .map(f64::to_bits)
        })
        .collect();
    let mut bytes = Vec::new();
    if let Some(buffer) = surface.narrow() {
        bytes.resize(count * 4, 0);
        buffer.output(0, &mut bytes);
    }
    Snapshot {
        result,
        work: context.work,
        pixels,
        bytes,
    }
}

#[test]
fn repeated_regions_preserve_pixels_and_work_in_every_storage_format() {
    let list = list(vec![
        Command::Glyphs(Box::new(run(Point::new(0.25, 0.5), 2))),
        Command::Glyphs(Box::new(run(Point::new(3.25, 3.5), 3))),
    ]);
    for device in [
        Transform::IDENTITY,
        Transform::new(0.0, 1.0, -1.0, 0.0, 8.0, 0.0),
        Transform::new(1.0, 0.125, 0.25, 0.75, 0.0, 0.0),
        Transform::new(-1.0, 0.0, 0.0, 1.0, 8.0, 0.0),
    ] {
        for format in [
            plan::Format::F64,
            plan::Format::N16,
            plan::Format::F32,
            plan::Format::U8,
        ] {
            let mut cached = context(&list, device, true);
            let mut uncached = context(&list, device, false);
            for tile in [
                tile(0, 0, 8, 2),
                tile(0, 2, 8, 2),
                tile(1, 4, 5, 3),
                tile(6, 7, 2, 1),
            ] {
                let expected = paint(&mut uncached, &list, tile, format);
                assert!(expected.result.is_ok(), "{expected:?}");
                assert_eq!(paint(&mut cached, &list, tile, format), expected);
            }
            assert!(cached.scene.get(0).is_some());
            assert!(cached.scene.get(1).is_some());
            assert_eq!(uncached.scene.get(0), None);
        }
    }
}

#[test]
fn warmed_off_tile_runs_preserve_every_work_threshold_and_error_context() {
    let list = list(vec![Command::Glyphs(Box::new(run(
        Point::new(0.25, 0.5),
        2,
    )))]);
    let outside = tile(6, 6, 2, 2);
    let mut probe = context(&list, Transform::IDENTITY, false);
    paint(&mut probe, &list, outside, plan::Format::F64)
        .result
        .unwrap();
    let before = probe.work;
    paint(&mut probe, &list, outside, plan::Format::F64)
        .result
        .unwrap();
    let cost = probe.work - before;
    let mut saw_positioned_error = false;
    for remaining in 0..=cost + 1 {
        let mut outcomes = Vec::new();
        for enabled in [false, true] {
            let mut context = context(&list, Transform::IDENTITY, enabled);
            paint(&mut context, &list, outside, plan::Format::F64)
                .result
                .unwrap();
            assert_eq!(context.scene.get(0).is_some(), enabled);
            context.limits.raster_work = context.work + remaining;
            outcomes.push(paint(&mut context, &list, outside, plan::Format::F64));
        }
        assert_eq!(outcomes[0], outcomes[1], "remaining={remaining}");
        if let Err(error) = &outcomes[0].result {
            saw_positioned_error |= error.object == Some(ObjectId::new(17, 0));
        }
    }
    assert!(saw_positioned_error);
}

#[test]
fn inactive_and_single_source_runs_do_not_publish_hulls() {
    let mut inactive = run(Point::new(0.25, 0.5), 2);
    inactive.paint.color = crate::color::Color::Pattern {
        selected: None,
        base: None,
    };
    let list = list(vec![
        Command::Glyphs(Box::new(inactive)),
        Command::Glyphs(Box::new(run(Point::new(2.25, 0.5), 1))),
        Command::Glyphs(Box::new(run(Point::new(4.25, 0.5), 2))),
    ]);
    let mut cached = context(&list, Transform::IDENTITY, true);
    let mut uncached = context(&list, Transform::IDENTITY, false);
    for region in [tile(0, 0, 8, 4), tile(0, 4, 8, 4)] {
        let expected = paint(&mut uncached, &list, region, plan::Format::F64);
        assert!(expected.result.is_ok());
        assert_eq!(
            paint(&mut cached, &list, region, plan::Format::F64),
            expected
        );
    }
    assert_eq!(cached.scene.get(0), None);
    assert_eq!(cached.scene.get(1), None);
    assert!(cached.scene.get(2).is_some());
}

#[test]
fn nested_and_view_groups_cannot_alias_root_ordinals() {
    for view in [
        None,
        Some(ViewFit {
            anchor: Point::new(0.0, 0.0),
            no_zoom: true,
            no_rotate: true,
        }),
    ] {
        let root = run(Point::new(5.0, 5.0), 2);
        let device = Transform::scale(1.5, 1.5);
        let expected_hull = root.device_hull(device);
        let list = list(vec![
            Command::Glyphs(Box::new(root)),
            Command::Group(Group {
                commands: vec![Command::Glyphs(Box::new(run(Point::new(0.25, 0.5), 2)))],
                bounds: None,
                position: position(100),
                attributes: Attributes {
                    view,
                    ..Attributes::default()
                },
                composite: Composite::opaque(),
            }),
        ]);
        let mut cached = context(&list, device, true);
        let mut uncached = context(&list, device, false);
        for region in [tile(0, 0, 8, 4), tile(0, 4, 8, 4)] {
            let expected = paint(&mut uncached, &list, region, plan::Format::F64);
            assert!(expected.result.is_ok(), "{expected:?}");
            assert_eq!(
                paint(&mut cached, &list, region, plan::Format::F64),
                expected
            );
            assert_eq!(cached.scene.get(0), Some(expected_hull));
            assert_eq!(cached.scene.get(1), None);
        }
    }
}

#[test]
fn changed_device_does_not_read_or_replace_the_canonical_hull() {
    let list = list(vec![Command::Glyphs(Box::new(run(
        Point::new(0.25, 0.5),
        2,
    )))]);
    let region = tile(0, 0, 8, 4);
    let mut cached = context(&list, Transform::IDENTITY, true);
    let mut uncached = context(&list, Transform::IDENTITY, false);
    assert_eq!(
        paint(&mut cached, &list, region, plan::Format::F64),
        paint(&mut uncached, &list, region, plan::Format::F64)
    );
    let canonical = cached.scene.get(0).unwrap();
    for device in [Transform::translate(5.0, 0.0), Transform::IDENTITY] {
        cached.device = device;
        uncached.device = device;
        assert_eq!(
            paint(&mut cached, &list, region, plan::Format::F64),
            paint(&mut uncached, &list, region, plan::Format::F64)
        );
        assert_eq!(cached.scene.get(0), Some(canonical));
    }
}

#[test]
fn large_origins_keep_relative_extents_and_numeric_range_errors() {
    let mut list = list(vec![Command::Glyphs(Box::new(run(
        Point::new(1e16, 0.5),
        2,
    )))]);
    list.crop = Rect::from_corners(1e16, 0.0, 1e16 + 8.0, 8.0).unwrap();
    let device = Transform::translate(-1e16, 0.0);
    let mut cached = context(&list, device, true);
    let mut uncached = context(&list, device, false);
    for region in [tile(0, 0, 4, 2), tile(0, 2, 4, 2), tile(0, 0, 4, 2)] {
        let expected = paint(&mut uncached, &list, region, plan::Format::F64);
        // The original rasterizer rejects this huge origin when painting.
        // Caching must preserve that error, not widen numeric admission.
        if region.y == 0 {
            let error = expected.result.as_ref().unwrap_err();
            assert_eq!(error.kind, pagura_core::ErrorKind::Limit);
            assert_eq!(error.message, "geometry exceeds numeric working range");
        }
        assert_eq!(
            paint(&mut cached, &list, region, plan::Format::F64),
            expected
        );
    }
    let [lo, hi] = cached.scene.get(0).unwrap();
    assert!(hi.x - lo.x >= 1.25);
}

#[test]
fn errors_after_a_cached_run_keep_the_same_source_order() {
    let mut invalid = run(Point::new(2.25, 0.5), 2);
    invalid.glyphs[0].source = u32::MAX;
    invalid.paint.position = position(200);
    let list = list(vec![
        Command::Glyphs(Box::new(run(Point::new(0.25, 0.5), 2))),
        Command::Glyphs(Box::new(invalid)),
    ]);
    let mut cached = context(&list, Transform::IDENTITY, true);
    let mut uncached = context(&list, Transform::IDENTITY, false);
    for _ in 0..2 {
        let expected = paint(&mut uncached, &list, tile(0, 0, 8, 4), plan::Format::F64);
        let error = expected.result.as_ref().unwrap_err();
        assert_eq!(error.kind, pagura_core::ErrorKind::InvalidArgument);
        assert_eq!(error.position, position(20));
        assert_eq!(error.object, Some(ObjectId::new(17, 0)));
        assert_eq!(
            paint(&mut cached, &list, tile(0, 0, 8, 4), plan::Format::F64),
            expected
        );
        assert!(cached.scene.get(0).is_some());
    }
}

#[test]
fn warmed_competitive_fragments_preserve_pixels_and_every_work_boundary() {
    // Narrow storage alone does not select fragments: use the Competitive raster
    // policy and the same fragment cache/resolution setup as render_competitive.
    fn prepared(list: &DisplayList, enabled: bool) -> Context {
        let options = RenderOptions {
            accuracy: crate::Accuracy::Competitive,
            ..RenderOptions::default()
        };
        let mut context = context(list, Transform::IDENTITY, enabled);
        context.raster = Policy::COMPETITIVE.rasterizer().unwrap();
        let mut cache = crate::fragments::GlyphCache::new(crate::fragments::CACHE_BYTES);
        cache.make_room(list.glyph_placements(), list.limits.document_bytes);
        context.fragments = Some(Fragments {
            cache,
            accumulator: crate::fragments::Accumulator::default(),
            resolution: crate::fragments::Resolution::new(&options),
            row: Vec::new(),
        });
        context
    }

    for format in [plan::Format::U8, plan::Format::N16] {
        let list = list(vec![Command::Glyphs(Box::new(run(
            Point::new(0.25, 0.5),
            2,
        )))]);
        let visible = tile(0, 0, 8, 4);
        let outside = tile(6, 6, 2, 2);
        let mut cached = prepared(&list, true);
        let mut uncached = prepared(&list, false);
        for (region, has_paint) in [(visible, true), (outside, false), (visible, true)] {
            let expected = paint(&mut uncached, &list, region, format);
            assert!(expected.result.is_ok(), "{expected:?}");
            assert_eq!(
                expected
                    .bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|pixel| *pixel != [77, 153, 230, 255]),
                has_paint,
            );
            assert_eq!(paint(&mut cached, &list, region, format), expected);
        }

        // Warm both the masks and the memo before varying the remaining budget.
        // Compare the complete result, positioned error, consumed work and partial
        // output; a warmed hull must not buy additional semantic work allowance.
        for region in [visible, outside] {
            let mut probe = prepared(&list, false);
            paint(&mut probe, &list, visible, format).result.unwrap();
            let before = probe.work;
            paint(&mut probe, &list, region, format).result.unwrap();
            let cost = probe.work - before;
            for remaining in 0..=cost + 1 {
                let mut outcomes = Vec::new();
                for enabled in [false, true] {
                    let mut context = prepared(&list, enabled);
                    paint(&mut context, &list, visible, format).result.unwrap();
                    context.limits.raster_work = context.work + remaining;
                    outcomes.push(paint(&mut context, &list, region, format));
                }
                assert_eq!(
                    outcomes[0], outcomes[1],
                    "region={region:?}, remaining={remaining}"
                );
                assert_eq!(outcomes[0].result.is_ok(), remaining >= cost);
            }
        }
    }
}
