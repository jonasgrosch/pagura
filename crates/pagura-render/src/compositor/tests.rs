use super::*;
use crate::RenderOptions;
use crate::display::DisplayList;
use crate::transparency::{Composite, MaskMethod, SoftMask};
use pagura_raster::{Pixmap, Point};

fn compiled(area: &str, file: &str) -> DisplayList {
    let document = pagura_document::Document::open(
        pagura_testkit::fixture(area, file),
        &pagura_document::OpenOptions::default(),
    )
    .unwrap();
    crate::compile_page(&document, document.page(0).unwrap()).unwrap()
}

#[test]
fn rendering_enforces_its_own_work_budget() {
    // Compiling within budget does not exempt the render from its own.
    let mut list = compiled("content", "group-i1-k0.pdf");
    list.limits.raster_work = 1;
    assert_eq!(
        list.render(&RenderOptions::default()).unwrap_err().kind,
        pagura_core::ErrorKind::Limit
    );
}

#[test]
fn simulation_requires_painted_colorants_and_is_shared_across_tiles() {
    for ((area, file), needed) in [
        (("overprint", "noop-rgb-multiply-on.pdf"), false),
        (("overprint", "fill-zero.pdf"), false),
        (("overprint", "image-ignores-nonzero-mode.pdf"), false),
        (("overprint", "shading-ignores-nonzero-mode.pdf"), false),
        (("overprint", "indexed-base-cmyk.pdf"), true),
        (("overprint", "separation-on.pdf"), true),
    ] {
        let list = compiled(area, file);
        let plan = list.separations().unwrap();
        assert_eq!(plan.enabled, needed);
        let options = RenderOptions::default();
        for x in 0..8 {
            list.render_tile(
                &options,
                Tile {
                    x,
                    y: 0,
                    width: 1,
                    height: 8,
                },
            )
            .unwrap();
            assert!(Arc::ptr_eq(&plan, &list.separations().unwrap()));
        }
    }
}

#[test]
fn icc_rgb_fallback_keeps_opaque_groups_eligible() {
    let list = compiled("transparency", "icc-source-and-group.pdf");
    assert!(!list.separations().unwrap().enabled);
    let group = list
        .commands
        .iter()
        .find_map(|command| match command {
            Command::Group(group) => Some(group),
            _ => None,
        })
        .unwrap();
    assert_eq!(group.attributes.model, Some(Model::Rgb));
    assert!(list.scene().normal);
    assert!(group.composite.simple());
}

#[test]
fn separation_batches_and_masks_preserve_tile_parity() {
    for (area, file) in [
        ("overprint", "spot-batches-6.pdf"),
        ("overprint", "spot-passes-rgb-group.pdf"),
        ("overprint", "spot-luminosity-mask-uses-alternate.pdf"),
        ("overprint", "fractional-coverage.pdf"),
        ("overprint", "combined-fill-stroke-half.pdf"),
    ] {
        let list = compiled(area, file);
        for background in [
            pagura_raster::Rgba8::WHITE,
            pagura_raster::Rgba8::TRANSPARENT,
        ] {
            let options = RenderOptions {
                scale: 1.375,
                background,
                ..RenderOptions::default()
            };
            let full = list.render(&options).unwrap();
            let (width, height) = list.device_size(&options).unwrap();
            for y in (0..height).step_by(4) {
                for x in (0..width).step_by(3) {
                    let tile = Tile {
                        x,
                        y,
                        width: 3.min(width - x),
                        height: 4.min(height - y),
                    };
                    let part = list.render_tile(&options, tile).unwrap();
                    for row in 0..tile.height as usize {
                        let start = ((y as usize + row) * width as usize + x as usize) * 4;
                        let stride = tile.width as usize * 4;
                        assert_eq!(
                            &part.data()[row * stride..(row + 1) * stride],
                            &full.data()[start..start + stride]
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn spot_surfaces_share_the_process_memory_allowance() {
    let mut list = compiled("overprint", "spot-batches-6.pdf");
    list.limits.decoded_bytes = 4096;
    assert_eq!(
        list.render(&RenderOptions::default()).unwrap_err().kind,
        pagura_core::ErrorKind::Limit
    );
}

#[test]
fn full_normal_stores_preserve_the_f64_evaluation() {
    let state = Composite::default();
    for alpha in [0.0, 1.0] {
        for backdrop in [-0.0, 0.0, 0.1, 0.5, 1.0] {
            for source in [-0.0, 0.0, f64::MIN_POSITIVE, 0.3, 0.5, 1.0] {
                let mut pixel = Pixel {
                    color: [backdrop; 4],
                    alpha,
                    group: 0.5,
                    shape: 0.25,
                };
                let opacity = 1.0;
                let expected = (1.0 - opacity) * backdrop
                    + opacity * ((1.0 - alpha) * source + alpha * source);
                frame(Backdrop::Transparent, false).paint(
                    &mut pixel,
                    Source::new([source; 4], 1.0, 1.0),
                    &state,
                    (0, 0),
                );
                assert_eq!(pixel.color.map(f64::to_bits), [expected.to_bits(); 4]);
                assert_eq!([pixel.alpha, pixel.group, pixel.shape], [1.0; 3]);
            }
        }
    }
}

#[test]
fn sole_rgb_images_preserve_work_and_surface_admission() {
    let mut list = compiled("paint", "image-page-rgb.pdf");
    let options = RenderOptions::default();
    let expected = list.render(&options).unwrap();
    // One surface visit, one clip visit, one sample per pixel, and one mark.
    list.limits.raster_work = 3 * 32 * 32 + 1;
    assert_eq!(list.render(&options).unwrap(), expected);
    list.limits.raster_work -= 1;
    let error = list.render(&options).unwrap_err();
    assert_eq!(error.kind, pagura_core::ErrorKind::Limit);
    assert!(matches!(
        error.position,
        pagura_core::Position::Stream { .. }
    ));
    for work in [1, 32 * 32] {
        list.limits.raster_work = work;
        let error = list.render(&options).unwrap_err();
        assert_eq!(error.kind, pagura_core::ErrorKind::Limit);
        assert_eq!(error.position, pagura_core::Position::Unknown);
    }
    list.limits.raster_work = Limits::default().raster_work;
    list.limits.decoded_bytes = 32 * 32 * size_of::<Pixel>();
    assert_eq!(list.render(&options).unwrap(), expected);
    list.limits.decoded_bytes -= 1;
    assert_eq!(list.render(&options).unwrap(), expected);
    list.limits.decoded_bytes = 1000;
    assert_eq!(
        list.render(&options).unwrap_err().kind,
        pagura_core::ErrorKind::Limit
    );
}

#[test]
fn only_sole_opaque_rgb_images_copy_straight_into_the_output() {
    use crate::display::ImageCommand;
    use crate::interpreter::images::Image;
    fn copies(edit: impl FnOnce(&mut ImageCommand)) -> bool {
        let mut list = compiled("paint", "image-page-rgb.pdf");
        let [Command::Image(image)] = list.commands.as_mut_slice() else {
            panic!("the fixture paints one image");
        };
        edit(image);
        let [Command::Image(image)] = list.commands.as_slice() else {
            unreachable!();
        };
        let options = RenderOptions::default();
        let separations = list.preview_separations(&options).unwrap();
        let plan = plan::page(&list, &separations).unwrap();
        let (mut context, model) = Context::page(&list, &options, &plan, separations).unwrap();
        let (width, height) = list.device_size(&options).unwrap();
        let tile = Tile {
            x: 0,
            y: 0,
            width,
            height,
        };
        let mut output =
            Pixmap::new_with_limits(width, height, options.background, list.limits).unwrap();
        context
            .copy_opaque_image(image, model, &mut output, tile, tile)
            .unwrap()
    }
    fn image(edit: impl FnOnce(&mut Image)) -> impl FnOnce(&mut ImageCommand) {
        |command| edit(Arc::get_mut(&mut command.image).expect("the image is unshared"))
    }
    let transfer = compiled("recovery", "transfer-image.pdf")
        .commands
        .iter()
        .find_map(|command| match command {
            Command::Image(image) => image.transfer.clone(),
            _ => None,
        })
        .expect("the fixture paints its image through a transfer");
    assert!(copies(|_| {}));
    assert!(!copies(image(|i| i.decode = vec![(1.0, 0.0); 3])));
    assert!(!copies(image(|i| i.decoded.interpolate = true)));
    assert!(!copies(|c| c.composite.alpha = 0.5));
    assert!(!copies(|c| c.transfer = Some(transfer)));
    // A fractional last column, and an image over half the chunk.
    assert!(!copies(|c| c.transform.a = 31.5));
    assert!(!copies(|c| c.transform.a = 16.0));
}

#[test]
fn solid_runs_preserve_unit_and_fractional_backdrop_alpha_evaluation() {
    let tile = Tile {
        x: 0,
        y: 0,
        width: 2,
        height: 1,
    };
    let color = [79.0 / 255.0, 0.125, 0.5, 0.0];
    let backdrops = [Pixel::default(), Pixel::source([0.25; 4], 1.0)];
    let mut surface = Surface::new(tile, backdrops.to_vec());
    assert!(surface.fill_solid(0, 2, color));
    let bits = |p: Pixel| {
        (
            p.color.map(f64::to_bits),
            [p.alpha, p.group, p.shape].map(f64::to_bits),
        )
    };
    let source = Source::new(color, 1.0, 1.0);
    for (pixel, mut expected) in surface.pixels().iter().zip(backdrops) {
        frame(Backdrop::Transparent, false).paint(
            &mut expected,
            source,
            &Composite::default(),
            (0, 0),
        );
        assert_eq!(bits(*pixel), bits(expected));
    }
    surface.pixels_mut()[1].alpha = 0.5;
    let before = surface.pixels()[0].color;
    // Fractional backdrops take the general f64 compositing expression.
    assert!(!surface.fill_solid(0, 2, [0.75; 4]));
    assert_eq!(
        surface.pixels()[0].color.map(f64::to_bits),
        before.map(f64::to_bits)
    );
}

#[test]
fn soft_mask_values_share_the_live_raster_memory_allowance() {
    let limits = Limits::default();
    let mut path = PathBuilder::new();
    path.rect(0.0, 0.0, 2.0, 2.0);
    let path = Arc::new(path.finish());
    let clip = Mask::from_path(2, 2, &path, Transform::IDENTITY, FillRule::NonZero, 4).unwrap();
    let mut context = Context::new(limits, Transform::IDENTITY, (*path).clone());
    // Rectangles take a closed form without sweep scratch; warm it with a triangle.
    let mut warm = PathBuilder::new();
    warm.move_to(Point::new(0.0, 0.0));
    warm.line_to(Point::new(2.0, 0.0)).unwrap();
    warm.line_to(Point::new(0.0, 2.0)).unwrap();
    warm.close();
    let mask = context
        .raster
        .clipped_path(
            &clip,
            Arc::new(warm.finish()),
            Transform::IDENTITY,
            FillRule::NonZero,
            limits,
        )
        .unwrap()
        .unwrap();
    context.raster.recycle(mask);
    let retained = context.raster.retained_bytes();
    assert!(retained >= 512);
    context.limits.decoded_bytes = retained + 64;
    context.memory = Meter::new(context.limits.decoded_bytes, "test");
    let mask = Arc::new(SoftMask {
        group: Group {
            position: pagura_core::Position::Unknown,
            bounds: None,
            commands: Vec::new(),
            attributes: crate::transparency::Attributes::default(),
            composite: Composite::default(),
        },
        method: MaskMethod::Alpha,
        backdrop: [0.0; 4],
        transfer: None,
    });
    let tile = Tile {
        x: 0,
        y: 0,
        width: 8,
        height: 8,
    };
    let values = context.soft_mask(&mask, tile, Model::Rgb, 0).unwrap();
    // An empty alpha-mask group contributes no opacity (11.6.5.1).
    assert_eq!(values.as_slice(), [0.0; 64]);
    assert_eq!(context.raster.retained_bytes(), 0);
    assert_eq!(context.memory.used(), 512);
    assert!(context.memory.used() <= context.limits.decoded_bytes);
    // The cached mask holds its bytes until it is replaced or discarded.
    context.discard_mask();
    assert_eq!(context.memory.used(), 0);
}

#[test]
fn surfaces_hold_their_bytes_until_released() {
    let limits = Limits {
        decoded_bytes: 4 * size_of::<Pixel>(),
        ..Limits::default()
    };
    let mut context = Context::new(limits, Transform::IDENTITY, PathBuilder::new().finish());
    let tile = Tile {
        x: 0,
        y: 0,
        width: 2,
        height: 2,
    };
    let surface = context.surface(tile, Backdrop::Transparent).unwrap();
    assert_eq!(context.memory.used(), 4 * size_of::<Pixel>());
    let Err(error) = context.surface(tile, Backdrop::Transparent) else {
        panic!("a second surface exceeds the budget");
    };
    assert_eq!(
        (error.kind, error.message),
        (
            pagura_core::ErrorKind::Limit,
            "group memory budget exceeded"
        )
    );
    context.release(surface);
    assert_eq!(context.memory.used(), 0);
    let surface = context.surface(tile, Backdrop::Transparent).unwrap();
    assert_eq!(context.memory.used(), 4 * size_of::<Pixel>());
    context.release(surface);
    // Scoped holdings return their bytes whatever the scope returns.
    let failed = context.holding(8, "held", |_| Err::<(), _>(Error::limit("inner")));
    assert_eq!(failed.unwrap_err().message, "inner");
    assert_eq!(context.memory.used(), 0);
}
fn glyph_context(limits: Limits) -> Context {
    let mut crop = PathBuilder::new();
    crop.rect(0.0, 0.0, 4.0, 4.0);
    Context::new(limits, Transform::IDENTITY, crop.finish())
}
fn glyph_run() -> crate::display::GlyphRun {
    let paint = PathCommand {
        path: Arc::default(),
        mark: Mark::Fill(FillRule::NonZero),
        clip: None,
        color: crate::color::Color::Solid {
            space: crate::color::Space::Rgb,
            components: vec![0.2, 0.5, 0.8].into(),
        },
        position: pagura_core::Position::Unknown,
        transfer: None,
        composite: Composite {
            alpha: 0.5,
            ..Default::default()
        },
    };
    let mut run = crate::display::GlyphRun::new(paint);
    for (width, x) in [(0.4, 0.0), (0.6, 0.2), (0.8, 0.4), (0.4, 100.0)] {
        let mut path = PathBuilder::new();
        path.rect(0.0, 0.0, width, 1.0);
        run.push(
            Arc::new(path.finish()),
            Transform::translate(x, 0.25),
            pagura_core::Position::Unknown,
        )
        .unwrap();
    }
    run
}
fn glyph_surface() -> Surface {
    let tile = Tile {
        x: 0,
        y: 0,
        width: 4,
        height: 4,
    };
    Surface::new(tile, vec![Pixel::source([0.3, 0.6, 0.9, 0.0], 1.0); 16])
}
#[test]
fn batched_spans_preserve_the_f64_fold_and_cache_blind_limits() {
    let mut run = glyph_run();
    let limits = Limits::default();
    let initial = Backdrop::Color([0.3, 0.6, 0.9, 0.0]);
    for blend in [
        crate::blend::BlendMode::Normal,
        crate::blend::BlendMode::Multiply,
        crate::blend::BlendMode::ColorDodge,
    ] {
        run.paint.composite.blend = blend;
        for knockout in [false, true] {
            let mut reference = glyph_surface();
            let mut context = glyph_context(limits);
            for &placement in &run.glyphs {
                let clip = context
                    .clip(run.paint.clip.as_ref(), reference.tile)
                    .unwrap();
                let geometry = run
                    .geometry(placement, Transform::IDENTITY, &limits)
                    .unwrap();
                if let Some(mask) = context
                    .raster
                    .clipped_glyph(&clip, geometry, placement.origin, limits)
                    .unwrap()
                {
                    context
                        .paint_path(&run.paint, &mask, &mut reference, frame(initial, knockout))
                        .unwrap();
                    context.raster.recycle(mask);
                }
            }
            for capacity in [0, 16 * 1024, limits.decoded_bytes] {
                let mut context = glyph_context(limits);
                context.glyphs = crate::display::GlyphCache::new(capacity);
                let mut actual = glyph_surface();
                context
                    .glyphs(&run, actual.tile, &mut actual, frame(initial, knockout))
                    .unwrap();
                for (a, b) in actual.pixels().iter().zip(reference.pixels()) {
                    assert_eq!(a.color.map(f64::to_bits), b.color.map(f64::to_bits));
                    assert_eq!(
                        [a.alpha, a.group, a.shape].map(f64::to_bits),
                        [b.alpha, b.group, b.shape].map(f64::to_bits)
                    );
                }
            }
        }
    }
    for work in [1, 4, 8, 16, 32, 64, 128, 256, 512, 1024] {
        let limits = Limits {
            raster_work: work,
            ..limits
        };
        let mut outcomes = Vec::new();
        for capacity in [0, 16 * 1024, limits.decoded_bytes] {
            let mut context = glyph_context(limits);
            context.glyphs = crate::display::GlyphCache::new(capacity);
            let mut target = glyph_surface();
            outcomes.push(
                context
                    .glyphs(&run, target.tile, &mut target, frame(initial, false))
                    .map_err(|e| (e.kind, e.position, e.object)),
            );
        }
        assert!(outcomes.windows(2).all(|pair| pair[0] == pair[1]));
    }
}

fn frame(initial: Backdrop<'_>, knockout: bool) -> Frame<'_> {
    Frame {
        initial,
        model: Model::Rgb,
        knockout,
        depth: 0,
    }
}
