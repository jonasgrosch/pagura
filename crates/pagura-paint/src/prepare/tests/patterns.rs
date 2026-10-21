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
    });
    cell
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
