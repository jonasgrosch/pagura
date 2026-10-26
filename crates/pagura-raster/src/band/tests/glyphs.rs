//! Translated glyph placements against placements that translate every
//! vertex first: the same bytes, within the work bound, at any band height.

mod oracle;

use super::*;
use crate::band::shape::Body;
use std::sync::Arc;

#[test]
fn a_fractional_clip_run_rounding_to_full_still_clips_glyph_geometry() {
    let limits = Limits::default();
    let area = area(0, 0, 3, 2, 16);
    let clip_path = polygon(&[
        Point::new(-2.0, 0.0),
        Point::new(3.0, 0.0),
        Point::new(4.0, 0.5),
        Point::new(3.0, 0.9985),
        Point::new(-2.0, 0.9985),
    ]);
    let mut mark = PathBuilder::new();
    mark.move_to(Point::new(-1.0, 0.4995));
    mark.cubic_to(
        Point::new(0.0, 0.4995),
        Point::new(1.0, 0.4995),
        Point::new(2.0, 0.4995),
    )
    .unwrap();
    mark.line_to(Point::new(2.0, 1.0)).unwrap();
    mark.line_to(Point::new(-1.0, 1.0)).unwrap();
    mark.close();
    let edges = Arc::new(GlyphEdges::new(&mark.finish(), [1.0, 0.0, 0.0, 1.0], &limits).unwrap());
    let prepare = |options: &Prep<'_>| glyph::place(&edges, Point::default(), options).unwrap();
    let clip = |options: &ShapeOptions<'_>| {
        ClipShape::new(
            &[(&clip_path, FillRule::NonZero)],
            Transform::IDENTITY,
            options,
        )
        .unwrap()
    };
    let coverage = invariant(&area, &prepare, Some(&clip));
    // The clip alone rounds .9985 to255, but its intersection with this
    // mark has area .9985-.4995=.499, whose coverage byte is127, not128.
    assert_eq!(coverage[0], 127);
    assert_eq!(coverage[1], 127);
}

fn outlines() -> Vec<Path> {
    let mut out = Vec::new();
    // An "o": two quadratic contours of opposite directions.
    let mut path = PathBuilder::new();
    for (r, sign) in [(500.0, 1.0), (300.0, -1.0)] {
        let mut current = Point::new(r, 0.0);
        path.move_to(current);
        for (control, end) in [
            (Point::new(r, sign * r), Point::new(0.0, sign * r)),
            (Point::new(-r, sign * r), Point::new(-r, 0.0)),
            (Point::new(-r, -sign * r), Point::new(0.0, -sign * r)),
            (Point::new(r, -sign * r), Point::new(r, 0.0)),
        ] {
            let elevate = |p: Point| p + (control - p) * (2.0 / 3.0);
            path.cubic_to(elevate(current), elevate(end), end).unwrap();
            current = end;
        }
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

/// Translated placements integrate the relative spline with the origin
/// added per row; moving the pieces first rounds differently, by far less
/// than a level, so the bytes agree but for rare ties.
#[test]
fn translated_placements_match_moved_pieces() {
    let limits = Limits::default();
    let area = area(0, 0, 48, 40, 16);
    let mut rng = Lcg(77);
    let mut crop = PathBuilder::new();
    crop.rect(0.0, 0.0, 48.0, 40.0);
    let crop = crop.finish();
    let (mut pixels, mut differing) = (0, 0);
    for outline in outlines() {
        for size in [0.011, 0.023, 0.04] {
            let linear = [size, 0.002, 0.0, -size];
            let edges = Arc::new(GlyphEdges::new(&outline, linear, &limits).unwrap());
            assert!(edges.relative().is_some());
            assert!(edges.curved());
            for _ in 0..12 {
                let origin = Point::new(rng.grid(2.0, 42.0, 4096), rng.grid(4.0, 38.0, 4096));
                for clipped in [false, true] {
                    let clip = clipped.then(|| {
                        ClipShape::new(
                            &[(&crop, FillRule::NonZero)],
                            Transform::IDENTITY,
                            &public(area, &limits),
                        )
                        .unwrap()
                    });
                    let prep = options(area, &limits, clip.as_ref());
                    let fast = Shape::glyph_in(&edges, origin, &prep).unwrap().unwrap();
                    let slow = glyph::moved(&edges, origin, &prep).unwrap().unwrap();
                    if !clipped {
                        assert!(matches!(fast.body, Body::Glyph(_)));
                    }
                    let (a, work) = raster(&area, Some(&fast), clip.as_ref());
                    let (b, _) = raster(&area, Some(&slow), clip.as_ref());
                    for (x, y) in a.iter().zip(&b) {
                        assert!(x.abs_diff(*y) <= 1, "origin {origin:?}");
                        differing += usize::from(x != y);
                    }
                    pixels += a.len();
                    assert!(work <= fast.work());
                }
                let prepare = |o: &Prep<'_>| Shape::glyph_in(&edges, origin, o).unwrap();
                invariant(&area, &prepare, None);
            }
        }
    }
    assert!(differing * 10_000 < pixels, "{differing} of {pixels}");
}

/// A run's glyphs merged at once give each glyph's own bytes, in chunks
/// when the run storage fills, within the glyphs' work.
#[test]
fn runs_merge_the_rows_of_glyphs_that_share_no_pixel() {
    let limits = Limits::default();
    let area = area(0, 0, 2000, 40, 16);
    let outlines = outlines();
    let mut placed = Vec::new();
    for (i, x) in (0..700).map(|i| (i, 1.3 + f64::from(i) * 2.71)) {
        let outline = &outlines[i as usize % outlines.len()];
        let linear = [0.0021, 0.0, 0.0, -0.0023];
        let edges = Arc::new(GlyphEdges::new(outline, linear, &limits).unwrap());
        let origin = Point::new(x, 20.37 + f64::from(i % 3) * 7.0);
        let prep = options(area, &limits, None);
        if let Some(shape) = glyph::place(&edges, origin, &prep).unwrap() {
            placed.push(shape);
        }
    }
    // Keep glyphs whose pixels another kept glyph does not reach.
    let mut kept: Vec<&Shape> = Vec::new();
    for glyph in &placed {
        let b = glyph.bounds();
        if kept.iter().all(|k| k.bounds().intersect(&b).is_empty()) {
            kept.push(glyph);
        }
    }
    assert!(kept.len() > 300, "{}", kept.len());
    let mut expected = vec![0_u8; 2000 * 40];
    for shape in &kept {
        for (e, v) in expected.iter_mut().zip(raster(&area, Some(shape), None).0) {
            *e = (*e).max(v);
        }
    }
    let mut bytes = Meter::new(usize::MAX, "bytes");
    let mut raster = BandRasterizer::new(area, &mut bytes).unwrap();
    let mut band = BandCoverage::with_capacity(Capacity::band(&area), &mut bytes).unwrap();
    let mut got = vec![0_u8; 2000 * 40];
    let keyed: Vec<(u32, &Shape)> = (0..).zip(kept.iter().copied()).collect();
    for b in area.bands() {
        let mut rest = &keyed[..];
        while !rest.is_empty() {
            let taken = raster.rasterize_run(rest, None, b, &mut band);
            assert!(taken >= 1);
            rest = &rest[taken..];
            add(&band, &mut got);
        }
    }
    assert_eq!(got, expected);
    let total: usize = kept.iter().map(|s| s.work()).sum();
    assert!(raster.work_done() <= total);
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

/// A stored quadratic arch with overlapping copies or many short pieces,
/// as degree-elevated cubics.
fn arches(copies: u32, segments: u32) -> Path {
    arches_of(copies, segments, false)
}

/// [`arches`], kept quadratic when `quadratic`.
fn arches_of(copies: u32, segments: u32, quadratic: bool) -> Path {
    let mut path = PathBuilder::new();
    for copy in 0..copies {
        let x = f64::from(copy) * 0.23;
        let mut previous = Point::new(x, 0.0);
        path.move_to(previous);
        for i in 0..segments {
            let lo = f64::from(i) / f64::from(segments);
            let hi = f64::from(i + 1) / f64::from(segments);
            let endpoint = Point::new(x + 8.0 * hi, 16.0 * hi * (1.0 - hi));
            let h = hi - lo;
            let control = previous + Point::new(4.0 * h, 8.0 * (1.0 - 2.0 * lo) * h);
            if quadratic {
                path.quad_to(control, endpoint).unwrap();
            } else {
                path.cubic_to(
                    previous + (control - previous) * (2.0 / 3.0),
                    endpoint + (control - endpoint) * (2.0 / 3.0),
                    endpoint,
                )
                .unwrap();
            }
            previous = endpoint;
        }
        path.close();
    }
    path.finish()
}

#[test]
fn stored_cubics_define_the_true_curve_oracle() {
    let mut bowl = PathBuilder::new();
    bowl.move_to(Point::new(0.0, 0.0));
    bowl.cubic_to(
        Point::new(1.0 / 3.0, 0.0),
        Point::new(2.0 / 3.0, 1.0 / 3.0),
        Point::new(1.0, 1.0),
    )
    .unwrap();
    bowl.line_to(Point::new(0.0, 1.0)).unwrap();
    bowl.close();
    let areas = oracle::areas(&bowl.finish(), Transform::IDENTITY, FillRule::NonZero);
    assert!((areas.at(0, 0) - 2.0 / 3.0).abs() < 1e-14);
    let arch = oracle::areas(&arches(1, 1), Transform::IDENTITY, FillRule::NonZero);
    assert!((arch.total() - 64.0 / 3.0).abs() < 1e-12);
}

#[test]
fn quadratic_placements_meet_the_true_curve_with_many_pieces_and_overlaps() {
    let limits = Limits::default();
    let area = area(0, 0, 32, 24, 16);
    let mut rng = Lcg(123);
    let mut maximum = 0.0_f64;
    for outline in [arches(1, 1), arches(1, 40), arches(2, 1)] {
        for linear in [
            [1.0, 0.0, 0.0, 1.0],
            [0.9, 0.17, -0.1, -1.2],
            [1.1, -0.3, 0.21, 0.8],
        ] {
            let edges = Arc::new(GlyphEdges::new(&outline, linear, &limits).unwrap());
            for _ in 0..4 {
                let origin = Point::new(rng.grid(8.0, 10.0, 4096), rng.grid(10.0, 12.0, 4096));
                let prep = options(area, &limits, None);
                let shape = glyph::place(&edges, origin, &prep).unwrap().unwrap();
                let (pixels, work) = raster(&area, Some(&shape), None);
                assert!(work <= shape.work());
                let [a, b, c, d] = linear;
                let exact = oracle::areas(
                    &outline,
                    Transform::new(a, b, c, d, origin.x, origin.y),
                    FillRule::NonZero,
                );
                for y in 0..24 {
                    for x in 0..32 {
                        let level = exact.at(x, y) * 255.0;
                        let value = pixels[y as usize * 32 + x as usize];
                        let error = (f64::from(value) - level).abs();
                        maximum = maximum.max(error);
                        assert!(
                            error <= 1.0,
                            "{x},{y}: {value} vs {level}, linear {linear:?}, origin {origin:?}"
                        );
                    }
                }
            }
        }
    }
    eprintln!("true-curve placement maximum error: {maximum:.9} levels");
}

#[test]
fn kept_quadratics_round_correctly_without_inspection() {
    // A single arch and many pieces integrate their own quadratics; the
    // overlapping copies integrate their covered set's, inspected. Every
    // byte is the correctly rounded true area, as the elevated cubics'
    // certified bytes.
    let limits = Limits::default();
    let area = area(0, 0, 32, 24, 16);
    let mut rng = Lcg(321);
    for (copies, segments) in [(1, 1), (1, 40), (2, 1)] {
        let quadratic = arches_of(copies, segments, true);
        let elevated = arches(copies, segments);
        for linear in [
            [1.0, 0.0, 0.0, 1.0],
            [0.9, 0.17, -0.1, -1.2],
            [1.1, -0.3, 0.21, 0.8],
        ] {
            let kept = Arc::new(GlyphEdges::new(&quadratic, linear, &limits).unwrap());
            assert!(kept.curved());
            let reduced = Arc::new(GlyphEdges::new(&elevated, linear, &limits).unwrap());
            for _ in 0..4 {
                let origin = Point::new(rng.grid(8.0, 10.0, 4096), rng.grid(10.0, 12.0, 4096));
                let prep = options(area, &limits, None);
                let shape = glyph::place(&kept, origin, &prep).unwrap().unwrap();
                let (pixels, work) = raster(&area, Some(&shape), None);
                assert!(work <= shape.work());
                let shape = glyph::place(&reduced, origin, &prep).unwrap().unwrap();
                assert_eq!(pixels, raster(&area, Some(&shape), None).0);
                let [a, b, c, d] = linear;
                let exact = oracle::areas(
                    &quadratic,
                    Transform::new(a, b, c, d, origin.x, origin.y),
                    FillRule::NonZero,
                );
                for y in 0..24 {
                    for x in 0..32 {
                        let level = exact.at(x, y) * 255.0;
                        let value = f64::from(pixels[y as usize * 32 + x as usize]);
                        assert!(
                            (value - level).abs() <= 0.5 + 1e-9,
                            "{x},{y}: {value} vs {level}, linear {linear:?}, origin {origin:?}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn clipped_quadratics_keep_the_true_curve_reference() {
    let limits = Limits::default();
    let area = area(0, 0, 32, 24, 16);
    let mut triangle = PathBuilder::new();
    triangle.polygon([
        Point::new(7.13, 5.72),
        Point::new(21.3, 17.4),
        Point::new(6.24, 19.81),
    ]);
    let triangle = triangle.finish();
    let clip = ClipShape::new(
        &[(&triangle, FillRule::NonZero)],
        Transform::IDENTITY,
        &public(area, &limits),
    )
    .unwrap();
    let mut maximum = 0.0_f64;
    for outline in [arches(1, 40), arches(2, 1)] {
        let linear = [1.2, 0.14, 0.2, -1.0];
        let origin = Point::new(7.37, 14.81);
        let edges = Arc::new(GlyphEdges::new(&outline, linear, &limits).unwrap());
        let prep = options(area, &limits, Some(&clip));
        let shape = glyph::place(&edges, origin, &prep).unwrap().unwrap();
        let (pixels, work) = raster(&area, Some(&shape), Some(&clip));
        assert!(work <= shape.work());
        let [a, b, c, d] = linear;
        let exact = oracle::clipped_areas(
            &outline,
            Transform::new(a, b, c, d, origin.x, origin.y),
            &triangle,
        );
        for y in 0..24 {
            for x in 0..32 {
                let level = exact.at(x, y) * 255.0;
                let value = pixels[y as usize * 32 + x as usize];
                let error = (f64::from(value) - level).abs();
                maximum = maximum.max(error);
                assert!(error <= 1.0, "{x},{y}: {value} vs {level}");
            }
        }
        let prepare = |p: &Prep<'_>| glyph::place(&edges, origin, p).unwrap();
        let clipping = |p: &ShapeOptions<'_>| {
            ClipShape::new(&[(&triangle, FillRule::NonZero)], Transform::IDENTITY, p).unwrap()
        };
        invariant(&area, &prepare, Some(&clipping));
    }
    eprintln!("true-curve clipped maximum error: {maximum:.9} levels");
}

#[test]
fn large_source_coordinates_and_linear_scales_keep_true_curve_coverage() {
    let limits = Limits::default();
    let area = area(0, 0, 32, 24, 16);
    let mut maximum = 0.0_f64;
    for copies in [1, 2] {
        for (anchor, scale) in [
            (0.0, 1.0),
            (1_073_741_824.0, 1_048_576.0),
            (549_755_813_888.0, 1_073_741_824.0),
            (-549_755_813_888.0, 1_073_741_824.0),
        ] {
            let outline = arches(copies, 40)
                .transformed(Transform::new(scale, 0.0, 0.0, scale, anchor, -anchor));
            let linear = [1.0 / scale, 0.0, 0.0, -1.0 / scale];
            let origin = Point::new(8.37 - anchor / scale, 12.29 - anchor / scale);
            let edges = Arc::new(GlyphEdges::new(&outline, linear, &limits).unwrap());
            let shape = glyph::place(&edges, origin, &options(area, &limits, None))
                .unwrap()
                .unwrap();
            let (pixels, work) = raster(&area, Some(&shape), None);
            assert!(work <= shape.work());
            let [a, b, c, d] = linear;
            let exact = oracle::areas(
                &outline,
                Transform::new(a, b, c, d, origin.x, origin.y),
                FillRule::NonZero,
            );
            for y in 0..24 {
                for x in 0..32 {
                    let value = pixels[y as usize * 32 + x as usize];
                    let level = exact.at(x, y) * 255.0;
                    let error = (f64::from(value) - level).abs();
                    maximum = maximum.max(error);
                    assert!(
                        error <= 1.0,
                        "{x},{y}: {value} vs {level}, anchor {anchor}, scale {scale}"
                    );
                }
            }
        }
    }
    eprintln!("large-source true-curve maximum error: {maximum:.9} levels");
}

#[test]
fn fallback_half_levels_keep_their_bytes_at_translated_origins_and_band_grids() {
    let limits = Limits::default();
    let mut path = PathBuilder::new();
    let h = 3.0 * 0.500_001 / 255.0;
    for _ in 0..2 {
        path.move_to(Point::default());
        path.cubic_to(
            Point::new(1.0 / 3.0, 2.0 * h / 3.0),
            Point::new(2.0 / 3.0, 2.0 * h / 3.0),
            Point::new(1.0, 0.0),
        )
        .unwrap();
        path.close();
    }
    // The two copies keep one covered set, whose bytes are certified.
    let edges = Arc::new(GlyphEdges::new(&path.finish(), [1.0, 0.0, 0.0, 1.0], &limits).unwrap());
    assert!(edges.curved());
    for offset in [0, 10_000] {
        let origin = Point::new(f64::from(offset), f64::from(offset));
        for height in [1, 3, 16, 256] {
            let area = area(offset, offset, offset + 1, offset + 1, height);
            let shape = Shape::glyph_in(&edges, origin, &options(area, &limits, None))
                .unwrap()
                .unwrap();
            assert_eq!(raster(&area, Some(&shape), None).0, [1]);
        }
    }
}

#[test]
fn translated_clip_half_levels_use_only_hulls_that_can_change_the_joint_area() {
    let limits = Limits::default();
    let mut path = PathBuilder::new();
    path.move_to(Point::new(-1.0, 0.0));
    path.cubic_to(
        Point::new(0.0, 1.0),
        Point::new(1.0, 1.0),
        Point::new(2.0, 0.0),
    )
    .unwrap();
    path.close();
    let edges = Arc::new(GlyphEdges::new(&path.finish(), [1.0, 0.0, 0.0, 1.0], &limits).unwrap());
    for offset in [0, 10_000] {
        let base = f64::from(offset);
        let left = if offset == 0 { -0.1 } else { 1.0 / 3.0 };
        let top = base + 0.3;
        let rectangle = polygon(&[
            Point::new(left, base - 0.1),
            Point::new(base + 1.1, base - 0.1),
            Point::new(base + 1.1, top),
            Point::new(left, top),
        ]);
        let trapezoid = polygon(&[
            Point::new(left, base - 0.1),
            Point::new(base + 1.1, base - 0.1),
            Point::new(base + 1.2, top),
            Point::new(left, top),
        ]);
        for clip_path in [&rectangle, &trapezoid] {
            for height in [1, 3, 16] {
                let area = area(offset, offset, offset + 1, offset + 1, height);
                let clip = ClipShape::new(
                    &[(clip_path, FillRule::NonZero)],
                    Transform::IDENTITY,
                    &public(area, &limits),
                )
                .unwrap();
                let shape = Shape::glyph_in(
                    &edges,
                    Point::new(base + 0.2, base),
                    &options(area, &limits, Some(&clip)),
                )
                .unwrap()
                .unwrap();
                assert_eq!(
                    raster(&area, Some(&shape), Some(&clip)).0,
                    [crate::quantize_unit(top - base)],
                );
            }
        }
    }
}
