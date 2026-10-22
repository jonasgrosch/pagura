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

/// A run's glyphs merged at once give each glyph's own bytes, in chunks
/// when the run storage fills, within the glyphs' work; binned masks give
/// the bytes of their shapes.
#[test]
fn runs_merge_the_rows_of_glyphs_that_share_no_pixel() {
    let limits = Limits::default();
    let area = area(0, 0, 2000, 40, 16);
    let outlines = outlines();
    for origins in [GlyphOrigins::Exact, GlyphOrigins::Binned] {
        let mut placed = Vec::new();
        // The render's placements and formed levels.
        let (mut placements, mut formed) = (Vec::new(), Vec::new());
        let scratch = &mut super::super::binned::Scratch::default();
        for (i, x) in (0..700).map(|i| (i, 1.3 + f64::from(i) * 2.71)) {
            let outline = &outlines[i as usize % outlines.len()];
            let linear = [0.0021, 0.0, 0.0, -0.0023];
            let edges = Arc::new(match origins {
                GlyphOrigins::Exact => GlyphEdges::new(outline, linear, &limits).unwrap(),
                GlyphOrigins::Binned => GlyphEdges::binned(outline, linear, 1.0, &limits).unwrap(),
            });
            let origin = Point::new(x, 20.37 + f64::from(i % 3) * 7.0);
            let prep = options(area, &limits, None);
            let mask = glyph::mask(
                &edges,
                origin,
                &prep,
                (&mut placements, &mut formed, scratch),
            )
            .unwrap()
            .then(|| placements.len() - 1);
            if let Some(shape) = glyph::place(&edges, origin, origins, &prep).unwrap() {
                placed.push((shape, mask));
            }
        }
        assert_eq!(
            placed.iter().all(|(_, mask)| mask.is_some()),
            origins == GlyphOrigins::Binned
        );
        // Keep glyphs whose pixels another kept glyph does not reach.
        let mut kept: Vec<&(Shape, Option<usize>)> = Vec::new();
        for glyph in &placed {
            let b = glyph.0.bounds();
            if kept.iter().all(|k| k.0.bounds().intersect(&b).is_empty()) {
                kept.push(glyph);
            }
        }
        assert!(kept.len() > 300, "{}", kept.len());
        let mut expected = vec![0_u8; 2000 * 40];
        for (shape, _) in &kept {
            for (e, v) in expected.iter_mut().zip(raster(&area, Some(shape), None).0) {
                *e = (*e).max(v);
            }
        }
        // Every other mask goes in as its shape.
        let run: Vec<RunGlyph<'_>> = kept
            .iter()
            .enumerate()
            .map(|(i, (shape, mask))| match mask {
                Some(mask) if i % 2 == 0 => RunGlyph::Mask(placements[*mask].view(&formed)),
                _ => RunGlyph::Shape(shape),
            })
            .collect();
        let mut bytes = Meter::new(usize::MAX, "bytes");
        let mut raster = BandRasterizer::new(area, &mut bytes).unwrap();
        let mut band = BandCoverage::with_capacity(Capacity::band(&area), &mut bytes).unwrap();
        let mut got = vec![0_u8; 2000 * 40];
        let mut alone = vec![0_u8; 2000 * 40];
        for b in area.bands() {
            let mut rest = &run[..];
            while !rest.is_empty() {
                let taken = raster.rasterize_run(rest, None, b, &mut band);
                assert!(taken >= 1);
                rest = &rest[taken..];
                add(&band, &mut got);
            }
            for glyph in &run {
                if let RunGlyph::Mask(mask) = glyph {
                    raster.rasterize_mask(*mask, b, &mut band);
                    add(&band, &mut alone);
                }
            }
        }
        assert_eq!(got, expected);
        let masks: usize = kept
            .iter()
            .filter_map(|(_, m)| *m)
            .map(|m| placements[m].view(&formed).work())
            .sum();
        let total: usize = kept.iter().map(|(s, _)| s.work()).sum();
        assert!(raster.work_done() <= total + masks);
        if origins == GlyphOrigins::Binned {
            for (i, (got, expected)) in alone.iter().zip(&expected).enumerate() {
                let row = i / 2000;
                let column = i % 2000;
                let by_mask = run.iter().any(|g| match g {
                    RunGlyph::Mask(m) => {
                        let b = m.bounds();
                        (b.y0..b.y1).contains(&(row as u32))
                            && (b.x0..b.x1).contains(&(column as u32))
                    }
                    RunGlyph::Shape(_) => false,
                });
                if by_mask {
                    assert_eq!(got, expected);
                }
            }
        }
    }
}

/// Adds the spans of `band`, in order, to `page` by maximum.
fn add(band: &BandCoverage, page: &mut [u8]) {
    let mut last = None;
    for row in band.rows() {
        assert!(last.is_none_or(|y| y < row.y));
        last = Some(row.y);
        let mut end = 0;
        for span in row.spans() {
            let (x, values) = match span {
                Span::Full { x, len } => (x, vec![255; len as usize]),
                Span::Partial { x, coverage } => (x, coverage.to_vec()),
            };
            assert!(x >= end, "spans in order");
            end = x + values.len() as u32;
            let start = row.y as usize * 2000 + x as usize;
            for (g, v) in page[start..start + values.len()].iter_mut().zip(values) {
                *g = (*g).max(v);
            }
        }
    }
}
