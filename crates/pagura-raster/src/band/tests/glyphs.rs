//! Translated glyph placements against placements that translate every
//! vertex first: the same bytes, within the work bound, at any band height.

use super::*;
use crate::band::shape::Body;
use std::sync::Arc;

fn outlines() -> Vec<Path> {
    let mut out = Vec::new();
    // An "o": two cubic contours of opposite directions (curve pieces).
    let mut path = PathBuilder::new();
    for (r, s) in [(500.0, 1.0), (300.0, -1.0)] {
        let k = 0.552_284_749_8 * r;
        path.move_to(Point::new(r, 0.0));
        path.cubic_to(
            Point::new(r, s * k),
            Point::new(k, s * r),
            Point::new(0.0, s * r),
        )
        .unwrap();
        path.cubic_to(
            Point::new(-k, s * r),
            Point::new(-r, s * k),
            Point::new(-r, 0.0),
        )
        .unwrap();
        path.cubic_to(
            Point::new(-r, -s * k),
            Point::new(-k, -s * r),
            Point::new(0.0, -s * r),
        )
        .unwrap();
        path.cubic_to(
            Point::new(k, -s * r),
            Point::new(r, -s * k),
            Point::new(r, 0.0),
        )
        .unwrap();
        path.close();
    }
    out.push(path.finish());
    // A polygonal "k" and a bar (a rectangle).
    out.push(polygon(&[
        Point::new(0.0, 0.0),
        Point::new(90.0, 0.0),
        Point::new(90.0, 300.0),
        Point::new(330.0, 0.0),
        Point::new(440.0, 0.0),
        Point::new(190.0, 310.0),
        Point::new(420.0, 700.0),
        Point::new(310.0, 700.0),
        Point::new(90.0, 360.0),
        Point::new(90.0, 720.0),
        Point::new(0.0, 720.0),
    ]));
    let mut bar = PathBuilder::new();
    bar.rect(0.0, 0.0, 500.0, 80.0);
    out.push(bar.finish());
    out
}

#[test]
fn translated_placements_match_translated_vertices() {
    let limits = Limits::default();
    let area = area(0, 0, 48, 40, 16);
    let mut rng = Lcg(77);
    let mut crop = PathBuilder::new();
    crop.rect(0.0, 0.0, 48.0, 40.0);
    let crop = crop.finish();
    for outline in outlines() {
        for size in [0.011, 0.023, 0.04] {
            let linear = [size, 0.002, 0.0, -size];
            let edges = Arc::new(GlyphEdges::new(&outline, linear, &limits).unwrap());
            assert!(edges.relative().is_some());
            for _ in 0..12 {
                let origin = Point::new(rng.grid(2.0, 42.0, 4096), rng.grid(4.0, 38.0, 4096));
                for clipped in [false, true] {
                    let clip = clipped.then(|| {
                        ClipShape::new(
                            &[(&crop, FillRule::NonZero)],
                            Transform::IDENTITY,
                            &public(area, &limits, Precision::Exact),
                        )
                        .unwrap()
                    });
                    let prep = options(area, &limits, clip.as_ref());
                    let fast = Shape::glyph_in(&edges, origin, GlyphOrigins::Exact, &prep).unwrap();
                    let slow = glyph::place_general(&edges, origin, &prep);
                    let (fast, slow) = (fast.unwrap(), slow.unwrap().unwrap());
                    if !clipped {
                        assert!(matches!(fast.body, Body::Glyph(_) | Body::Rect(_)));
                    }
                    let (a, work) = raster(&area, Some(&fast), clip.as_ref());
                    let (b, _) = raster(&area, Some(&slow), clip.as_ref());
                    assert_eq!(a, b, "origin {origin:?}");
                    assert!(work <= fast.work());
                }
                let prepare =
                    |o: &Prep<'_>| Shape::glyph_in(&edges, origin, GlyphOrigins::Exact, o).unwrap();
                invariant(&area, &prepare, None);
            }
        }
    }
}
