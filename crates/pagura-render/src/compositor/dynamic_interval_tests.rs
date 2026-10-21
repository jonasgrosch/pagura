//! Dynamic F32 stores conservatively cover the surface after operation setup.
use super::*;
use crate::Accuracy;
use pagura_core::{Diagnostic, ErrorKind, Position, Recovery};
use pagura_document::{Document, OpenOptions};
use pagura_raster::Rgba8;

const FULL: Tile = Tile {
    x: 0,
    y: 0,
    width: 8,
    height: 8,
};

fn fixture(name: &str, alpha: f64) -> DisplayList {
    let document = Document::open(
        pagura_testkit::fixture("content", &format!("{name}.pdf")),
        &OpenOptions {
            recovery: Recovery::Strict,
            ..OpenOptions::default()
        },
    )
    .unwrap();
    let mut list = crate::compile_page(&document, document.page(0).unwrap()).unwrap();
    assert_eq!(document.diagnostics(), [] as [Diagnostic; 0]);
    let [command] = list.commands.as_mut_slice() else {
        panic!("the fixture must contain one dynamic paint command");
    };
    match command {
        Command::Image(image) => image.composite.alpha = alpha,
        Command::Shading(shading) => shading.composite.alpha = alpha,
        Command::Path(path) => path.composite.alpha = alpha,
        _ => panic!("unexpected dynamic fixture command"),
    }
    list
}

fn setup(list: &DisplayList, accuracy: Accuracy, tile: Tile) -> (Context, Surface, plan::Format) {
    let separations = Arc::new(Separations::default());
    let plan = match accuracy {
        Accuracy::Exact => plan::page(list, &separations),
        Accuracy::Competitive => plan::page_with_shading_patterns(list, &separations),
    }
    .unwrap();
    let options = RenderOptions {
        accuracy,
        ..RenderOptions::default()
    };
    let mut context = Context::new(list, &options, &plan, separations).unwrap();
    if accuracy == Accuracy::Competitive {
        context.raster = Policy::COMPETITIVE.rasterizer().unwrap();
    }
    let surface = context
        .page_surface(tile, plan.format, [255; 4], false)
        .unwrap();
    (context, surface, plan.format)
}

fn execute(context: &mut Context, list: &DisplayList, surface: &mut Surface) -> Result<()> {
    context.commands(
        &list.commands,
        surface,
        Backdrop::Transparent,
        Model::Rgb,
        false,
        0,
    )
}

fn output(context: &mut Context, surface: &Surface) -> Vec<u8> {
    let tile = surface.tile;
    let mut output = Pixmap::new(tile.width, tile.height, Rgba8::WHITE, 64).unwrap();
    let counters = (context.work, context.allocated, context.function_work);
    context
        .write_output(surface, &[], Model::Rgb, &mut output, tile)
        .unwrap();
    assert_eq!(
        (context.work, context.allocated, context.function_work),
        counters
    );
    output.data().to_vec()
}

fn assert_range(surface: &Surface, format: plan::Format, painted: bool) {
    if format == plan::Format::F32 {
        let count = (surface.tile.width * surface.tile.height) as usize;
        assert_eq!(
            surface.narrow().unwrap().painted_range(),
            Some(if painted { 0..count } else { count..0 })
        );
    } else {
        assert_eq!(format, plan::Format::F64);
        assert!(surface.narrow().is_none());
    }
}

fn assert_command_error(error: &Error, command: &Command, message: &str) {
    assert_eq!(error.kind, ErrorKind::Limit);
    assert_eq!(error.message, message);
    assert_eq!(error.position, command.position());
    let Position::Stream { object, .. } = command.position() else {
        panic!("the fixture must retain its content-stream position");
    };
    assert_eq!(error.object, Some(object));
    assert_eq!(error.repair, None);
}

#[test]
fn dynamic_consumers_mark_full_and_keep_independent_half_alpha_pixels() {
    for (name, evaluations) in [
        ("bounds-image", 0),
        ("bounds-shading", 16),
        ("pattern-anchored", 32),
    ] {
        let list = fixture(name, 0.5);
        let mut expected = vec![255; 8 * 8 * 4];
        for y in 0..8 {
            for x in 0..8 {
                // Integer PDF bounds, inverted page y; no raster-derived oracle.
                // Pattern t=(x+1/2)/8, then half-alpha source over white.
                let pixel = match name {
                    "bounds-image" if (1..5).contains(&x) && (3..5).contains(&y) => {
                        [128, 128, 255, 255]
                    }
                    "bounds-shading" if (2..6).contains(&x) && (3..7).contains(&y) => {
                        [255, 128, 128, 255]
                    }
                    "pattern-anchored" if (2..6).contains(&x) => [
                        [215, 199, 183, 167][x - 2],
                        128,
                        [167, 183, 199, 215][x - 2],
                        255,
                    ],
                    _ => [255; 4],
                };
                expected[(y * 8 + x) * 4..(y * 8 + x + 1) * 4].copy_from_slice(&pixel);
            }
        }
        for accuracy in [Accuracy::Exact, Accuracy::Competitive] {
            let (mut context, mut surface, format) = setup(&list, accuracy, FULL);
            assert_eq!(
                format,
                if name == "pattern-anchored" && accuracy == Accuracy::Exact {
                    plan::Format::F64
                } else {
                    plan::Format::F32
                }
            );
            assert_range(&surface, format, false);
            let allocated = context.allocated;
            execute(&mut context, &list, &mut surface).unwrap();
            assert_range(&surface, format, true);
            assert_eq!(context.allocated, allocated);
            assert_eq!(context.function_work, evaluations);
            assert_eq!(
                output(&mut context, &surface),
                expected,
                "{name} {accuracy:?}"
            );
            let options = RenderOptions {
                accuracy,
                ..RenderOptions::default()
            };
            assert_eq!(
                list.render(&options).unwrap().data(),
                expected,
                "public {name} {accuracy:?}"
            );
        }
    }
}

#[test]
fn disjoint_dynamic_operations_leave_the_interval_empty() {
    let tile = Tile {
        width: 1,
        height: 1,
        ..FULL
    };
    for name in ["bounds-image", "bounds-shading", "pattern-anchored"] {
        let list = fixture(name, 0.5);
        for accuracy in [Accuracy::Exact, Accuracy::Competitive] {
            let (mut context, mut surface, format) = setup(&list, accuracy, tile);
            let allocated = context.allocated;
            execute(&mut context, &list, &mut surface).unwrap();
            assert_range(&surface, format, false);
            assert_eq!(context.allocated, allocated);
            assert_eq!(context.function_work, 0);
            assert_eq!(
                output(&mut context, &surface),
                [255; 4],
                "{name} {accuracy:?}"
            );
        }
    }
}

#[test]
fn empty_shading_samples_still_conservatively_mark_the_dynamic_operation() {
    // The zero-length axial vector has no sample anywhere, despite a full clip.
    let list = fixture("shading-axial-empty", 0.5);
    for accuracy in [Accuracy::Exact, Accuracy::Competitive] {
        let (mut context, mut surface, format) = setup(&list, accuracy, FULL);
        assert_eq!(format, plan::Format::F32);
        let before = (context.work, context.allocated);
        execute(&mut context, &list, &mut surface).unwrap();
        assert_range(&surface, format, true);
        assert_eq!(
            (context.work, context.allocated),
            (before.0 + 1 + 64, before.1)
        );
        assert_eq!(context.function_work, 0);
        assert_eq!(output(&mut context, &surface), vec![255; 256]);
    }
}

#[test]
fn image_area_charge_failure_precedes_dynamic_preparation() {
    let list = fixture("bounds-image", 0.5);
    for accuracy in [Accuracy::Exact, Accuracy::Competitive] {
        let (mut context, mut surface, format) = setup(&list, accuracy, FULL);
        // Leave one command charge, but not the subsequent image mask-area
        // charge, which can include an integral fringe around the eight pixels.
        // A high seed keeps the rasterizer's separate geometry allowance ample.
        context.work = 1_000_000;
        context.limits.raster_work = context.work + 1;
        let allocated = context.allocated;
        let error = execute(&mut context, &list, &mut surface).unwrap_err();
        assert_command_error(
            &error,
            &list.commands[0],
            "compositing work budget exceeded",
        );
        assert_range(&surface, format, false);
        assert_eq!(
            (context.work, context.allocated, context.function_work),
            (1_000_001, allocated, 0)
        );
        assert_eq!(output(&mut context, &surface), vec![255; 256]);
    }
}

#[test]
fn late_function_failure_keeps_full_range_partial_pixels_and_charges() {
    for (name, charged) in [("shading-axial", 65), ("pattern-shading", 74)] {
        let list = fixture(name, 1.0);
        for accuracy in [Accuracy::Exact, Accuracy::Competitive] {
            let (mut context, mut surface, format) = setup(&list, accuracy, FULL);
            // Compile with ordinary limits first; the pattern has four operators.
            // One exponential-vector evaluation per pixel, exactly two remain.
            context.function_work = list.limits.operations - 2;
            let before = (context.work, context.allocated);
            let error = execute(&mut context, &list, &mut surface).unwrap_err();
            assert_command_error(
                &error,
                &list.commands[0],
                "function evaluation budget exceeded",
            );
            assert_range(&surface, format, true);
            assert_eq!(context.function_work, list.limits.operations);
            // Shading: command + 64 samples. Pattern: command + hull lookup +
            // five segments + 64 mask pixels + three attempted sample charges.
            assert_eq!(
                (context.work, context.allocated),
                (before.0 + charged, before.1)
            );
            let mut expected = vec![255; 256];
            // t=1/16 and 3/16; round(255*[1-t,0,t]), alpha=1.
            expected[..8].copy_from_slice(&[239, 0, 16, 255, 207, 0, 48, 255]);
            assert_eq!(
                output(&mut context, &surface),
                expected,
                "{name} {accuracy:?}"
            );
        }
    }
}
