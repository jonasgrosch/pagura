//! Tiling patterns (8.7.3): instances by exact geometry, coloured and
//! uncoloured, against the reference.

use crate::testing::{Build, check, mark};
use pagura_color::{ColorSpace, Components};
use pagura_core::Rect;
use pagura_interpret::{
    Color, GroupAttributes, GroupId, GroupRole, Paint, PatternId, StateId, Tiling, TilingColors,
    TilingPaint,
};
use pagura_raster::Transform;
use std::sync::Arc;

fn pattern(b: &mut Build, colors: TilingColors, matrix: Transform, step: [f64; 2]) -> GroupId {
    let cell = b.group(
        GroupRole::Tiling,
        GroupAttributes::default(),
        mark(StateId(0)),
    );
    b.scene.tilings.push(Tiling {
        cell,
        bbox: Rect::from_corners(0.0, 0.0, 2.0, 2.0).unwrap_or_else(|| unreachable!()),
        step,
        matrix,
        colors,
        spacing: pagura_interpret::TilingSpacing::NoDistortion,
    });
    cell
}

fn opaque_cover_scene(value: f64, outer_alpha: f64, copies: usize) -> Build {
    let mut b = Build::new(8.0, 8.0);
    let cell = pattern(
        &mut b,
        TilingColors::Colored,
        Transform::IDENTITY,
        [0.001; 2],
    );
    b.scene.tilings[0].bbox =
        Rect::from_corners(0.0, 0.0, 10.0, 10.0).unwrap_or_else(|| unreachable!());
    let black = b.color(ColorSpace::DeviceGray, &[value]);
    b.rect(cell, [0.0, 0.0, 10.0, 10.0], black, mark(StateId(0)));
    let tiles = b.paint(Paint::Tiling(TilingPaint {
        pattern: PatternId(0),
        color: None,
    }));
    let state = b.state(|state| state.alpha = outer_alpha);
    for _ in 0..copies {
        b.rect(GroupId(0), [0.0, 0.0, 8.0, 8.0], tiles, mark(state));
    }
    b
}

#[test]
fn opaque_cover_composites_outer_alpha_once() {
    for (alpha, level) in [(1.0, 0), (0.5, 128)] {
        let b = opaque_cover_scene(0.0, alpha, 1);
        let expected = [level, level, level, 255].repeat(64);
        assert_eq!(b.render_page(), expected);
    }
}

#[test]
fn opaque_cover_preserves_nonblack_and_translucent_cell_limits() {
    let mut b = opaque_cover_scene(0.25, 1.0, 1);
    let request = b.request(b.page(), 3);
    assert_eq!(
        b.prepare(&request, &pagura_core::Limits::default())
            .unwrap_err()
            .kind,
        pagura_core::ErrorKind::Limit
    );
    b.scene.paints[0] = Arc::new(Paint::Solid(Color {
        space: Arc::new(ColorSpace::DeviceGray),
        components: Components::from_slice(&[0.0]),
    }));
    b.scene.states[0] = Arc::new(pagura_interpret::Composite {
        alpha: 0.5,
        ..pagura_interpret::Composite::default()
    });
    assert_eq!(
        b.prepare(&request, &pagura_core::Limits::default())
            .unwrap_err()
            .kind,
        pagura_core::ErrorKind::Limit
    );
}

#[test]
fn opaque_cover_repeated_proofs_share_the_render_work_budget() {
    let pixel = Rect::from_corners(0.0, 0.0, 1.0, 1.0).unwrap();
    let limited = pagura_core::Limits {
        render_work: 1000,
        ..pagura_core::Limits::default()
    };
    let mut direct = Build::new(1.0, 1.0);
    let black = direct.color(ColorSpace::DeviceGray, &[0.0]);
    for _ in 0..16 {
        direct.rect(GroupId(0), [0.0, 0.0, 8.0, 8.0], black, mark(StateId(0)));
    }
    let request = direct.request(direct.page(), 3);
    assert!(
        direct.prepare(&request, &limited).is_ok(),
        "ordinary sixteen-mark pixel work fits"
    );
    let mut single = opaque_cover_scene(0.0, 1.0, 1);
    single.scene.page.crop = pixel;
    assert!(
        single.prepare(&request, &limited).is_ok(),
        "one metered proof fits"
    );
    let mut b = opaque_cover_scene(0.0, 1.0, 16);
    b.scene.page.crop = pixel;
    assert!(b.prepare(&request, &pagura_core::Limits::default()).is_ok());
    assert_eq!(
        b.prepare(&request, &limited).unwrap_err().kind,
        pagura_core::ErrorKind::Limit
    );
}

#[test]
fn opaque_cover_keeps_selected_color_functions_outside_the_certificate() {
    let function = Arc::new(
        pagura_color::Function::exponential(pagura_color::ExponentialFunction {
            domain: [0.0, 1.0],
            range: None,
            c0: vec![0.375],
            c1: vec![0.375],
            exponent: 1.0,
        })
        .unwrap(),
    );
    for selected in ["BG", "UCR"] {
        let mut b = opaque_cover_scene(0.0, 1.0, 1);
        let mut state = pagura_interpret::Composite::default();
        match selected {
            "BG" => state.conversion.black_generation = Some(Arc::clone(&function)),
            _ => state.conversion.undercolor_removal = Some(Arc::clone(&function)),
        }
        b.scene.states[0] = Arc::new(state);
        let request = b.request(b.page(), 3);
        let error = b
            .prepare(&request, &pagura_core::Limits::default())
            .unwrap_err();
        assert_eq!(error.kind, pagura_core::ErrorKind::Limit);
        assert_eq!(error.message, "tiling instance budget exceeded");
    }
}

#[test]
fn opaque_cover_validates_raw_points_before_small_maps_and_clips() {
    for oversized in ["none", "fill", "clip", "bbox"] {
        let mut b = opaque_cover_scene(0.0, 1.0, 1);
        b.scene.page.crop = Rect::from_corners(0.0, 0.0, 1.0, 1.0).unwrap();
        let box_size = if oversized == "bbox" { 1e13 } else { 1e12 };
        b.scene.tilings[0].bbox = Rect::from_corners(0.0, 0.0, box_size, box_size).unwrap();
        b.scene.tilings[0].step = [1.0; 2];
        b.scene.tilings[0].matrix = Transform::scale(1e-9, 1e-9);
        let clip = (oversized == "clip").then(|| b.clip(None, [0.0, 0.0, 1e13, 1e13]));
        let fill_size = if oversized == "fill" { 1e13 } else { 1e12 };
        let pagura_interpret::Node::Fill(fill) = &mut b.scene.nodes[0] else {
            unreachable!()
        };
        fill.path = crate::testing::rect_path([0.0, 0.0, fill_size, fill_size]);
        fill.mark.clip = clip;
        let request = b.request(b.page(), 3);
        if oversized == "none" {
            assert!(b.prepare(&request, &pagura_core::Limits::default()).is_ok());
            assert_eq!(b.render_page(), [0, 0, 0, 255]);
        } else {
            let error = b
                .prepare(&request, &pagura_core::Limits::default())
                .unwrap_err();
            assert_eq!(error.kind, pagura_core::ErrorKind::Limit);
            assert_eq!(error.message, "tiling instance budget exceeded");
        }
    }
}

#[test]
fn coloured_tiles_repeat_their_cell() {
    for (matrix, step) in [
        (Transform::translate(0.25, 0.5), [2.0, 2.0]),
        (Transform::new(1.5, 0.0, 0.0, 1.25, 0.75, 0.0), [2.5, 1.75]),
        (Transform::IDENTITY, [-2.0, 3.0]),
    ] {
        let mut b = Build::new(10.0, 8.0);
        let cell = pattern(&mut b, TilingColors::Colored, matrix, step);
        let red = b.rgb([0.9, 0.1, 0.1]);
        let half = b.state(|s| s.alpha = 0.5);
        b.rect(cell, [0.25, 0.25, 1.5, 1.25], red, mark(StateId(0)));
        b.rect(cell, [1.0, 0.75, 2.0, 2.0], red, mark(half));
        let tiles = b.paint(Paint::Tiling(TilingPaint {
            pattern: PatternId(0),
            color: None,
        }));
        let green = b.rgb([0.1, 0.8, 0.3]);
        b.rect(GroupId(0), [0.0, 0.0, 6.0, 8.0], green, mark(StateId(0)));
        let translucent = b.state(|s| s.alpha = 0.75);
        b.rect(GroupId(0), [1.25, 0.5, 9.5, 7.25], tiles, mark(translucent));
        check(&b);
    }
}

#[test]
fn uncoloured_tiles_take_the_use_colour() {
    let mut b = Build::new(8.0, 6.0);
    let cell = pattern(
        &mut b,
        TilingColors::Uncolored,
        Transform::IDENTITY,
        [2.0, 2.0],
    );
    let ignored = b.rgb([0.0, 1.0, 0.0]);
    b.rect(cell, [0.5, 0.5, 1.5, 1.75], ignored, mark(StateId(0)));
    let tiles = b.paint(Paint::Tiling(TilingPaint {
        pattern: PatternId(0),
        color: Some(Color {
            space: Arc::new(ColorSpace::DeviceRgb),
            components: Components::from_slice(&[0.2, 0.3, 0.9]),
        }),
    }));
    b.rect(GroupId(0), [0.75, 0.25, 7.5, 5.5], tiles, mark(StateId(0)));
    check(&b);
}
