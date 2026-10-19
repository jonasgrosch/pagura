use super::glyphs::Placement;
use super::*;
use pagura_core::Position;
use pagura_raster::Rasterizer;
use pagura_raster::{PathBuilder, Point};

fn paint(alpha: f64) -> PathCommand {
    PathCommand {
        path: Arc::default(),
        mark: Mark::Fill(FillRule::NonZero),
        clip: None,
        color: Color::default(),
        position: Position::Unknown,
        transfer: None,
        composite: crate::transparency::Composite {
            alpha,
            ..Default::default()
        },
    }
}
fn run(width: f64, origins: &[f64], alpha: f64) -> GlyphRun {
    let mut path = PathBuilder::new();
    path.rect(0.0, 0.0, width, 1.0);
    let path = Arc::new(path.finish());
    let mut run = GlyphRun::new(paint(alpha));
    for &x in origins {
        run.push(
            Arc::clone(&path),
            Transform::translate(x, 0.0),
            Position::Unknown,
        )
        .unwrap();
    }
    run
}
fn list(run: GlyphRun, knockout: bool) -> DisplayList {
    let commands = vec![Command::Glyphs(Box::new(run))];
    let commands = if knockout {
        vec![Command::Group(crate::transparency::Group {
            position: Position::Unknown,
            commands,
            bounds: None,
            attributes: crate::transparency::Attributes {
                knockout: true,
                ..Default::default()
            },
            composite: crate::transparency::Composite::default(),
        })]
    } else {
        commands
    };
    DisplayList {
        commands,
        crop: Rect::from_corners(0.0, 0.0, 2.0, 1.0).unwrap(),
        rotation: Rotation::None,
        user_unit: 1.0,
        limits: Limits::default(),
        group: crate::transparency::Attributes::default(),
        print_intent: false,
        separations: OnceLock::new(),
        scene: OnceLock::new(),
    }
}
#[test]
fn a_glyph_transform_that_overflows_in_device_space_is_a_limit() {
    let mut path = PathBuilder::new();
    path.rect(0.0, 0.0, 1.0, 1.0);
    let path = Arc::new(path.finish());
    for alpha in [1.0, 0.5] {
        let mut run = GlyphRun::new(paint(alpha));
        run.push(
            Arc::clone(&path),
            Transform::new(1e308, 0.0, 0.0, 1e308, 0.0, 0.0),
            Position::Unknown,
        )
        .unwrap();
        let error = list(run, false)
            .render(&RenderOptions::default().with_scale(4.0))
            .unwrap_err();
        assert_eq!(
            error.kind,
            pagura_core::ErrorKind::Limit,
            "alpha {alpha}: {error}"
        );
    }
}
#[test]
fn every_glyph_placement_pays_for_its_outline() {
    // One 400-vertex outline placed 1000 times: the geometry is cached,
    // but each placement still scans all of it.
    let mut path = PathBuilder::new();
    path.move_to(Point::new(0.0, 0.0));
    for i in 1..400 {
        let t = f64::from(i) / 400.0 * std::f64::consts::TAU;
        path.line_to(Point::new(0.25 + 0.25 * t.cos(), 0.5 + 0.25 * t.sin()))
            .unwrap();
    }
    path.close();
    let path = Arc::new(path.finish());
    for alpha in [1.0, 0.5] {
        let mut run = GlyphRun::new(paint(alpha));
        for i in 0..1000 {
            run.push(
                Arc::clone(&path),
                Transform::translate(f64::from(i % 4) * 0.25, 0.0),
                Position::Unknown,
            )
            .unwrap();
        }
        let mut list = list(run, false);
        list.render(&RenderOptions::default()).unwrap();
        list.limits.raster_work = 100_000;
        let error = list.render(&RenderOptions::default()).unwrap_err();
        assert_eq!(error.kind, pagura_core::ErrorKind::Limit, "alpha {alpha}");
    }
}
#[test]
fn placements_off_the_tile_are_never_flattened() {
    // As above, but every placement lies right of the two-pixel page: each
    // costs one bounds test instead of its 400-vertex scan, on either path.
    let mut path = PathBuilder::new();
    path.move_to(Point::new(0.0, 0.0));
    for i in 1..400 {
        let t = f64::from(i) / 400.0 * std::f64::consts::TAU;
        path.line_to(Point::new(0.25 + 0.25 * t.cos(), 0.5 + 0.25 * t.sin()))
            .unwrap();
    }
    path.close();
    let path = Arc::new(path.finish());
    for alpha in [1.0, 0.5] {
        let mut run = GlyphRun::new(paint(alpha));
        for i in 0..1000 {
            run.push(
                Arc::clone(&path),
                Transform::translate(8.0 + f64::from(i % 4) * 0.25, 0.0),
                Position::Unknown,
            )
            .unwrap();
        }
        let mut list = list(run, false);
        let expected = list.render(&RenderOptions::default()).unwrap();
        list.limits.raster_work = 100_000;
        assert_eq!(
            list.render(&RenderOptions::default()).unwrap(),
            expected,
            "alpha {alpha}"
        );
    }
}
#[test]
fn device_bounds_preserve_small_extents_at_large_user_origins() {
    let mut path = PathBuilder::new();
    path.rect(0.0, 0.0, 0.4, 1.0);
    let mut run = GlyphRun::new(paint(1.0));
    run.push(
        Arc::new(path.finish()),
        Transform::new(1e-5, 0.0, 0.0, 1e-5, 5e11, 0.0),
        Position::Unknown,
    )
    .unwrap();
    let device = Transform::new(1e6, 0.0, 0.0, 1e6, -5e17, 0.0);
    let placement = run.glyphs[0];
    // L = 10I and t = 0. Adding the user origin first erases the width.
    let hull = run
        .source(placement)
        .unwrap()
        .device_hull(placement.origin, device);
    assert!(hull[0].x <= 0.0 && hull[1].x >= 4.0);
    assert!(hull[0].y <= 0.0 && hull[1].y >= 10.0);
    let run_hull = run.device_hull(device);
    assert!(run_hull[0].x <= 0.0 && run_hull[1].x >= 4.0);
    let mut list = list(run, true);
    list.crop = Rect::from_corners(5e11, 0.0, 5e11 + 0.001, 0.001).unwrap();
    let image = crate::compositor::render(
        &list,
        &RenderOptions::from_dpi(72e6),
        Tile {
            x: 2,
            y: 995,
            width: 1,
            height: 1,
        },
    )
    .unwrap();
    // The canonical rectangle covers this whole pixel; the backdrop is white.
    assert_eq!(image.data(), [0, 0, 0, 255]);
}

#[test]
fn shared_columns_are_individual_marks_in_content_order() {
    let tile = Tile {
        x: 0,
        y: 0,
        width: 2,
        height: 1,
    };
    for (width, origins, alpha, knockout, expected) in [
        (0.4, [0.0, 0.6], 1.0, false, 92),
        (0.6, [0.2, 0.2], 1.0, false, 41),
        (0.4, [0.0, 0.6], 0.5, false, 163),
        (0.4, [0.0, 0.6], 0.5, true, 173),
    ] {
        let list = list(run(width, &origins, alpha), knockout);
        assert_eq!(list.len(), 2);
        let image = crate::compositor::render(&list, &RenderOptions::default(), tile).unwrap();
        assert_eq!(&image.data()[..4], &[expected, expected, expected, 255]);
        for x in 0..2 {
            let part = crate::compositor::render(
                &list,
                &RenderOptions::default(),
                Tile {
                    x,
                    y: 0,
                    width: 1,
                    height: 1,
                },
            )
            .unwrap();
            assert_eq!(
                part.data(),
                &image.data()[usize::try_from(x).unwrap() * 4..usize::try_from(x + 1).unwrap() * 4]
            );
        }
    }
}
#[test]
fn cache_hits_eviction_and_disabled_cache_keep_geometry_and_limits() {
    let run = run(0.4, &[0.0, 0.6], 1.0);
    let limits = Limits::default();
    let mut cache = GlyphCache::new(limits.decoded_bytes);
    let mut raster = Rasterizer::default();
    let first = cache
        .geometry(
            &run,
            run.glyphs[0],
            Transform::IDENTITY,
            &limits,
            &mut raster,
        )
        .unwrap();
    let second = cache
        .geometry(
            &run,
            run.glyphs[1],
            Transform::IDENTITY,
            &limits,
            &mut raster,
        )
        .unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    assert!(cache.retained_bytes() <= cache.maximum);
    cache.clear();
    cache.maximum = 0;
    let cold = cache
        .geometry(
            &run,
            run.glyphs[0],
            Transform::IDENTITY,
            &limits,
            &mut raster,
        )
        .unwrap();
    assert_eq!(first, cold);
    assert_eq!(cache.retained_bytes(), 0);
    for bytes in [
        first.retained_bytes() - 1,
        first.retained_bytes(),
        limits.decoded_bytes,
    ] {
        let limits = Limits {
            decoded_bytes: bytes,
            ..limits
        };
        let cold = cache.geometry(
            &run,
            run.glyphs[0],
            Transform::IDENTITY,
            &limits,
            &mut raster,
        );
        cache.maximum = limits.decoded_bytes;
        let warm = cache.geometry(
            &run,
            run.glyphs[0],
            Transform::IDENTITY,
            &limits,
            &mut raster,
        );
        assert_eq!(
            cold.as_ref().map(|g| g.contours()).map_err(|e| e.kind),
            warm.as_ref().map(|g| g.contours()).map_err(|e| e.kind)
        );
        cache.clear();
    }
    assert_eq!(size_of::<Placement>(), 24);
}
#[test]
fn geometry_keys_preserve_signed_zero_in_the_linear_map() {
    let mut run = run(0.4, &[0.0], 1.0);
    let outline = Arc::clone(&run.sources[0].outline);
    let signed = Transform::new(1.0, -0.0, -0.0, 1.0, 0.0, 0.0);
    run.push(outline, signed, Position::Unknown).unwrap();
    assert_ne!(run.glyphs[0].source, run.glyphs[1].source);
    let limits = Limits::default();
    let mut cache = GlyphCache::new(limits.decoded_bytes);
    let mut raster = Rasterizer::default();
    let positive = cache
        .geometry(
            &run,
            run.glyphs[1],
            Transform::IDENTITY,
            &limits,
            &mut raster,
        )
        .unwrap();
    let negative = cache
        .geometry(&run, run.glyphs[1], signed, &limits, &mut raster)
        .unwrap();
    assert!(!Arc::ptr_eq(&positive, &negative));
    let repeated = cache
        .geometry(&run, run.glyphs[1], signed, &limits, &mut raster)
        .unwrap();
    assert!(Arc::ptr_eq(&negative, &repeated));
}
#[test]
fn geometry_keys_distinguish_linear_maps_and_keep_outline_identities_alive() {
    let first_run = run(0.4, &[0.0], 1.0);
    let limits = Limits::default();
    let outline = Arc::downgrade(&first_run.sources[0].outline);
    let mut cache = GlyphCache::new(limits.decoded_bytes);
    let mut raster = Rasterizer::default();
    let first = cache
        .geometry(
            &first_run,
            first_run.glyphs[0],
            Transform::IDENTITY,
            &limits,
            &mut raster,
        )
        .unwrap();
    let scaled = cache
        .geometry(
            &first_run,
            first_run.glyphs[0],
            Transform::scale(-2.0, 1.0),
            &limits,
            &mut raster,
        )
        .unwrap();
    assert_ne!(first, scaled);
    drop(first_run);
    assert!(outline.upgrade().is_some());
    cache.clear();
    assert!(outline.upgrade().is_none());
    assert_eq!(first.contours()[0].points()[1], Point::new(0.4, 0.0));
}

#[test]
fn one_scene_classification_serves_the_opaque_renderer_and_the_compositor() {
    use crate::transparency::{Attributes, Composite, Group, Model};
    use scene::Scene;
    let group = |commands, isolated, knockout, model| {
        Command::Group(Group {
            position: Position::Unknown,
            commands,
            bounds: None,
            attributes: Attributes {
                model,
                isolated,
                knockout,
                ..Default::default()
            },
            composite: Composite::default(),
        })
    };
    let fill = || Command::Path(paint(1.0));
    let page = Attributes::default();
    let scene = |commands: &[Command]| {
        let scene = Scene::of(commands, &page);
        (scene.opaque, scene.normal)
    };
    // Opaque knockout reduces to source-over but stays off the 8-bit path.
    assert_eq!(
        scene(&[group(vec![fill(), fill()], false, true, None)]),
        (false, true)
    );
    let translucent = group(vec![Command::Path(paint(0.5))], false, true, None);
    assert_eq!(scene(&[translucent]), (false, false));
    // A non-isolated group composites in its parent's space, whatever its own.
    let inherited = group(vec![fill()], false, false, Some(Model::Cmyk));
    assert_eq!(scene(&[inherited]), (true, true));
    let converted = group(vec![fill()], true, false, Some(Model::Cmyk));
    assert_eq!(scene(&[converted]), (false, false));
    // Translucent Normal marks are not opaque; nesting does not hide a mark.
    let nested = group(
        vec![group(vec![Command::Path(paint(0.5))], false, false, None)],
        false,
        false,
        None,
    );
    assert_eq!(scene(&[fill(), nested]), (false, true));
    // A knockout page group needs group surfaces for everything.
    let knockout = Attributes {
        knockout: true,
        ..Default::default()
    };
    assert_eq!(
        Scene::of(&[fill()], &knockout),
        Scene {
            opaque: false,
            normal: false
        }
    );
}
