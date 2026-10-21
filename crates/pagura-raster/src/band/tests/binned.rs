//! Binned glyph placements: invariance, reuse and clips. Byte identity with
//! the old row-fragment masks was checked against `pagura-render`'s
//! fragment cache on the bundled fonts (the old cache is private there).

use super::*;
use std::sync::Arc;

fn outline() -> Path {
    // An "o": two cubic contours of opposite directions.
    let mut path = PathBuilder::new();
    let circle = |path: &mut PathBuilder, r: f64, reverse: bool| {
        let k = 0.552_284_749_8 * r;
        let s = if reverse { -1.0 } else { 1.0 };
        path.move_to(Point::new(r, 0.0));
        path.cubic_to(Point::new(r, s * k), Point::new(k, s * r), Point::new(0.0, s * r))
            .unwrap();
        path.cubic_to(Point::new(-k, s * r), Point::new(-r, s * k), Point::new(-r, 0.0))
            .unwrap();
        path.cubic_to(Point::new(-r, -s * k), Point::new(-k, -s * r), Point::new(0.0, -s * r))
            .unwrap();
        path.cubic_to(Point::new(k, -s * r), Point::new(r, -s * k), Point::new(r, 0.0))
            .unwrap();
        path.close();
    };
    circle(&mut path, 500.0, false);
    circle(&mut path, 300.0, true);
    path.finish()
}

#[test]
fn binned_placements_are_invariant_and_reuse_identical_masks() {
    let limits = Limits::default();
    let outline = outline();
    let area = area(0, 0, 64, 48, 16);
    for (scale, size) in [(1.0, 0.012), (2.0, 0.01), (4.0, 0.006)] {
        let linear = [size * scale, 0.0, 0.0, -size * scale];
        let edges = Arc::new(GlyphEdges::binned(&outline, linear, scale, &limits).unwrap());
        for origin in [Point::new(20.3, 30.7), Point::new(20.30001, 30.70002), Point::new(41.9, 17.05)] {
            let prepare =
                |options: &Prep<'_>| Shape::glyph_in(&edges, origin, GlyphOrigins::Binned, options).unwrap();
            let first = invariant(&area, &prepare, None);
            assert!(first.iter().any(|&v| v == 255) && first.iter().any(|&v| v > 0 && v < 255));
            // One rasterizer placing the glyph twice reuses its mask.
            let shape = prepare(&options(area, &limits, None)).unwrap();
            let mut bytes = Meter::new(usize::MAX, "bytes");
            let mut raster = BandRasterizer::new(area, &mut bytes).unwrap();
            let mut band = BandCoverage::with_capacity(shape.capacity(), &mut bytes).unwrap();
            let mut runs = Vec::new();
            for _ in 0..2 {
                let mut rows = Vec::new();
                for b in shape.bands(&area) {
                    raster.rasterize(
                        MarkCoverage {
                            shape: Some(&shape),
                            clip: None,
                        },
                        b,
                        &mut band,
                    );
                    rows.extend(band.rows().map(|r| (r.y, r.spans().map(|s| format!("{s:?}")).collect::<Vec<_>>())));
                }
                runs.push(rows);
            }
            assert_eq!(runs[0], runs[1]);
            assert!(raster.work_done() <= 2 * shape.work());
        }
    }
}

#[test]
fn binned_placements_multiply_their_clips() {
    let limits = Limits::default();
    let outline = outline();
    let area = area(0, 0, 64, 48, 8);
    let edges = Arc::new(GlyphEdges::binned(&outline, [0.03, 0.0, 0.0, -0.03], 2.0, &limits).unwrap());
    let origin = Point::new(30.4, 24.6);
    let unclipped = invariant(
        &area,
        &|o| Shape::glyph_in(&edges, origin, GlyphOrigins::Binned, o).unwrap(),
        None,
    );
    let mut rect = PathBuilder::new();
    rect.rect(20.5, 10.25, 12.0, 30.0);
    let rect = rect.finish();
    let triangle = polygon(&[Point::new(5.0, 5.0), Point::new(60.0, 20.0), Point::new(25.0, 45.0)]);
    for precision in [Precision::Exact, Precision::Approximate] {
        for clips in [vec![(&rect, FillRule::NonZero)], vec![(&triangle, FillRule::NonZero)]] {
            let clip = |o: &ShapeOptions<'_>| ClipShape::new(&clips, Transform::IDENTITY, o).unwrap();
            let clipped = invariant_with(
                &area,
                &|o| Shape::glyph_in(&edges, origin, GlyphOrigins::Binned, o).unwrap(),
                Some(&clip),
                precision,
            );
            let clip_alone = {
                let c = clip(&public(area, &limits, precision));
                raster(&area, None, Some(&c)).0
            };
            for ((&g, &c), &v) in unclipped.iter().zip(&clip_alone).zip(&clipped) {
                let expected = u8::try_from((u32::from(g) * u32::from(c) + 127) / 255).unwrap();
                assert!(v.abs_diff(expected) <= 1, "{v} for {g} × {c}");
            }
        }
    }
}
