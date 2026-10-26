//! Orchestration: device sizes, regions, pixmaps, band stops and page
//! ownership, on scenes built by hand.

use super::*;
use crate::testing::{SceneBuilder, document, media_box};
use pagura_core::{ErrorKind, Value};
use pagura_raster::Rgba8;
use std::sync::Arc;

fn context(entries: &[(&[u8], Value)]) -> RenderContext {
    RenderContext::new(Arc::new(document(entries)))
}

/// The empty scene of the context's first page.
fn empty_scene(context: &RenderContext) -> Scene {
    let page = context.document().page(0).unwrap();
    let mut scene = SceneBuilder::new(page.crop_box()).scene;
    scene.page.rotation = page.rotation();
    scene.page.user_unit = page.user_unit();
    scene
}

#[test]
fn device_size_follows_crop_box_rotation_user_unit_and_scale() {
    let rotated = document(&[
        media_box(200, 100),
        (b"Rotate", Value::Integer(90)),
        (b"UserUnit", Value::Integer(2)),
    ]);
    let page = rotated.page(0).unwrap();
    assert_eq!(
        device_size(page, &RenderOptions::default()).unwrap(),
        (200, 400)
    );
    assert_eq!(
        device_size(page, &RenderOptions::from_dpi(36.0)).unwrap(),
        (100, 200)
    );
    let fractional = RenderOptions {
        scale: 0.3,
        ..RenderOptions::default()
    };
    assert_eq!(device_size(page, &fractional).unwrap(), (60, 120));
    for scale in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let options = RenderOptions {
            scale,
            ..RenderOptions::default()
        };
        assert_eq!(
            device_size(page, &options).unwrap_err().kind,
            ErrorKind::InvalidArgument
        );
    }
}

#[test]
fn an_empty_scene_renders_its_background_whole_and_by_tile() {
    let context = context(&[media_box(40, 24)]);
    let scene = empty_scene(&context);
    let options = RenderOptions {
        background: Rgba8 {
            r: 255,
            g: 0,
            b: 0,
            a: 128,
        },
        ..RenderOptions::from_dpi(144.0)
    };
    let page = context.render_scene(&scene, &options, None).unwrap();
    assert_eq!((page.width(), page.height()), (80, 48));
    // The background is premultiplied once.
    assert!(page.data().chunks(4).all(|pixel| pixel == [128, 0, 0, 128]));
    let tile = Tile {
        x: 70,
        y: 40,
        width: 10,
        height: 8,
    };
    let part = context.render_scene(&scene, &options, Some(tile)).unwrap();
    assert_eq!((part.width(), part.height()), (10, 8));
    assert!(part.data().chunks(4).all(|pixel| pixel == [128, 0, 0, 128]));
}

#[test]
fn regions_outside_the_page_and_invalid_scales_fail_before_rendering() {
    let context = context(&[media_box(40, 24)]);
    let scene = empty_scene(&context);
    let options = RenderOptions::default();
    for tile in [
        Tile {
            x: 30,
            y: 0,
            width: 11,
            height: 1,
        },
        Tile {
            x: 0,
            y: 0,
            width: 0,
            height: 1,
        },
    ] {
        let error = context
            .render_scene(&scene, &options, Some(tile))
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::InvalidArgument, "{tile:?}");
    }
    let zero = RenderOptions {
        scale: 0.0,
        ..options
    };
    assert_eq!(
        context.render_scene(&scene, &zero, None).unwrap_err().kind,
        ErrorKind::InvalidArgument
    );
}

#[test]
fn the_pixel_budget_bounds_the_region() {
    let limits = pagura_core::Limits {
        pixels: 100,
        ..pagura_core::Limits::default()
    };
    let document = document(&[media_box(40, 24)]);
    let bytes = document.materialize().unwrap().bytes().to_vec();
    let options = pagura_document::OpenOptions {
        limits,
        ..pagura_document::OpenOptions::default()
    };
    let small = Document::open(bytes, &options).unwrap();
    let context = RenderContext::new(Arc::new(small));
    let scene = empty_scene(&context);
    let error = context
        .render_scene(&scene, &RenderOptions::default(), None)
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
    let tile = Tile {
        x: 0,
        y: 0,
        width: 10,
        height: 10,
    };
    assert!(
        context
            .render_scene(&scene, &RenderOptions::default(), Some(tile))
            .is_ok()
    );
}

#[test]
fn band_stepping_stops_between_bands_and_leaves_the_background() {
    let context = context(&[media_box(16, 100)]);
    let scene = empty_scene(&context);
    let options = RenderOptions {
        background: Rgba8::BLACK,
        ..RenderOptions::default()
    };
    let mut rows = Vec::new();
    let (pixmap, completion) = context
        .render_until(&scene, &options, None, |row| {
            rows.push(row);
            false
        })
        .unwrap();
    assert_eq!(completion, Completion::Complete);
    assert_eq!(rows.first(), Some(&0));
    assert!(rows.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!((pixmap.width(), pixmap.height()), (16, 100));
    let second = rows[1];
    let (pixmap, completion) = context
        .render_until(&scene, &options, None, |row| row == second)
        .unwrap();
    assert_eq!(completion, Completion::Stopped { row: second });
    assert_eq!((pixmap.width(), pixmap.height()), (16, 100));
    let (_, completion) = context
        .render_until(&scene, &options, None, |_| true)
        .unwrap();
    assert_eq!(completion, Completion::Stopped { row: 0 });
}

#[test]
fn pages_of_another_document_are_rejected() {
    let context = context(&[media_box(40, 24)]);
    let other = document(&[media_box(40, 24)]);
    let page = other.page(0).unwrap();
    let options = RenderOptions::default();
    for result in [
        context.scene(page).map(drop),
        context.record(page).map(drop),
        context.render_page(page, &options).map(drop),
        context.text_page(page).map(drop),
        context.content_bounds(page).map(drop),
    ] {
        assert_eq!(
            result.unwrap_err().as_error().unwrap().kind,
            ErrorKind::InvalidArgument
        );
    }
}

#[test]
fn a_store_without_capacity_retains_nothing() {
    let document = Arc::new(document(&[media_box(40, 24)]));
    let context = RenderContext::with_store_capacity(document, None, 0);
    let scene = empty_scene(&context);
    context
        .render_scene(&scene, &RenderOptions::default(), None)
        .unwrap();
    assert_eq!(context.retained_bytes(), 0);
}
