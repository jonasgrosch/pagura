use super::content_bounds;
use crate::testing::{SceneBuilder, rect};
use pagura_core::{ErrorKind, Position, Rect};
use pagura_interpret::{GlyphSource, GlyphsNode, MarkInfo, Node, OutlineKey, Placement};
use pagura_raster::{Path, PathBuilder, Point, Transform};
use std::sync::Arc;

const DEPTH: usize = 8;

fn page() -> SceneBuilder {
    SceneBuilder::new(rect(0.0, 0.0, 100.0, 80.0))
}

fn bounds(builder: &SceneBuilder) -> Option<Rect> {
    content_bounds(&builder.scene, DEPTH).unwrap()
}

fn square(x0: f64, y0: f64, x1: f64, y1: f64) -> Path {
    Path::rect(rect(x0, y0, x1, y1))
}

#[test]
fn an_empty_page_has_no_bounds() {
    assert_eq!(bounds(&page()), None);
}

#[test]
fn fills_are_their_hulls_clamped_to_the_crop_box() {
    let mut b = page();
    let paint = b.rgb();
    let root = b.scene.root;
    b.fill(root, square(8.0, 8.0, 24.0, 24.0), paint, None);
    assert_eq!(bounds(&b), Some(rect(8.0, 8.0, 24.0, 24.0)));
    b.fill(root, square(90.0, 70.0, 140.0, 120.0), paint, None);
    assert_eq!(bounds(&b), Some(rect(8.0, 8.0, 100.0, 80.0)));
}

#[test]
fn marks_that_paint_nothing_and_marks_clipped_away_add_nothing() {
    let mut b = page();
    let root = b.scene.root;
    let none = b.invisible();
    b.fill(root, square(8.0, 8.0, 24.0, 24.0), none, None);
    assert_eq!(bounds(&b), None);
    let paint = b.rgb();
    let clip = b.clip(None, rect(50.0, 50.0, 60.0, 60.0));
    b.fill(root, square(8.0, 8.0, 24.0, 24.0), paint, Some(clip));
    assert_eq!(bounds(&b), None);
    let inner = b.clip(Some(clip), rect(55.0, 0.0, 100.0, 80.0));
    b.fill(root, square(40.0, 40.0, 70.0, 70.0), paint, Some(inner));
    assert_eq!(bounds(&b), Some(rect(55.0, 50.0, 60.0, 60.0)));
}

#[test]
fn strokes_grow_by_their_pen_and_points_by_a_unit() {
    let mut b = page();
    let root = b.scene.root;
    let paint = b.rgb();
    let pen = b.pen(4.0, Transform::scale(2.0, 1.0));
    let mut line = PathBuilder::new();
    line.move_to(Point::new(20.0, 20.0));
    line.line_to(Point::new(40.0, 20.0)).unwrap();
    b.stroke(root, line.finish(), pen, paint);
    // Half the width, stretched by the pen (twice along x), times the
    // default miter limit of 10, then clamped to the crop box.
    let (x, y) = (2.0 * 2.0 * 10.0, 2.0 * 10.0);
    assert_eq!(bounds(&b), Some(rect(0.0, 20.0 - y, 40.0 + x, 20.0 + y)));
    let mut dot = page();
    let root = dot.scene.root;
    let paint = dot.rgb();
    let mut point = PathBuilder::new();
    point.move_to(Point::new(30.0, 30.0));
    point.close();
    dot.fill(root, point.finish(), paint, None);
    assert_eq!(bounds(&dot), Some(rect(29.0, 29.0, 31.0, 31.0)));
}

#[test]
fn groups_are_bounded_by_their_bounding_box() {
    let mut b = page();
    let root = b.scene.root;
    let paint = b.rgb();
    let group = b.group(root, Some(rect(0.0, 0.0, 30.0, 30.0)));
    b.fill(group, square(10.0, 10.0, 50.0, 50.0), paint, None);
    assert_eq!(bounds(&b), Some(rect(10.0, 10.0, 30.0, 30.0)));
    let open = b.group(root, None);
    b.fill(open, square(60.0, 5.0, 70.0, 15.0), paint, None);
    assert_eq!(bounds(&b), Some(rect(10.0, 5.0, 70.0, 30.0)));
}

#[test]
fn nesting_deeper_than_the_depth_budget_is_a_limit_error() {
    let mut b = page();
    let paint = b.rgb();
    let mut parent = b.scene.root;
    for _ in 0..DEPTH {
        parent = b.group(parent, None);
    }
    b.fill(parent, square(1.0, 1.0, 2.0, 2.0), paint, None);
    let error = content_bounds(&b.scene, DEPTH).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
    assert!(content_bounds(&b.scene, DEPTH + 1).unwrap().is_some());
}

#[test]
fn glyph_runs_cover_every_placement_and_grow_by_a_unit() {
    let mut b = page();
    let root = b.scene.root;
    let paint = b.rgb();
    let source = GlyphSource {
        outline: Arc::new(square(0.0, 0.0, 500.0, 700.0)),
        key: OutlineKey {
            font: pagura_interpret::FontKey(1),
            glyph: 3,
        },
        linear: Transform::scale(0.01, 0.01),
    };
    let placements = [Point::new(10.0, 20.0), Point::new(30.0, 40.0)]
        .map(|origin| Placement { source: 0, origin })
        .to_vec();
    b.node(
        root,
        Node::Glyphs(GlyphsNode {
            sources: vec![source],
            placements,
            positions: Vec::new(),
            paint,
            mark: MarkInfo {
                state: pagura_interpret::StateId(0),
                clip: None,
                position: Position::Unknown,
            },
        }),
    );
    assert_eq!(bounds(&b), Some(rect(9.0, 19.0, 36.0, 48.0)));
}
