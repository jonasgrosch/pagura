#![forbid(unsafe_code)]
#![expect(
    clippy::unwrap_used,
    reason = "fixture and setup helpers fail loudly on malformed expected data"
)]
//! Public-API invariant tests: composition, strokes, budgets and mask intersection.

use pagura_core::{ErrorKind, Limits, Rect, Result};
use pagura_raster::{
    FillRule, IntRect, LineCap, LineJoin, Mask, Paint, Path, PathBuilder, Pixmap, Point,
    Rasterizer, Rgba8, Stroke, Transform, stroke_outline,
};
use std::sync::Arc;

/// The pixels `x..x + width` × `y..y + height`.
fn area(x: u32, y: u32, width: u32, height: u32) -> IntRect {
    IntRect::new(x, y, width, height).unwrap()
}

/// Default limits with a pixel budget of exactly `pixels`.
fn pixels(pixels: usize) -> Limits {
    Limits {
        pixels,
        ..Limits::default()
    }
}

/// A mask of `path` over `area`, with a pixel budget of exactly its size.
fn mask(area: IntRect, path: &Path, rule: FillRule) -> Mask {
    let budget = pixels(usize::try_from(area.width() * area.height()).unwrap());
    Mask::from_path(area, path.clone(), Transform::IDENTITY, rule, budget).unwrap()
}

/// A transparent `size` × `size` surface with a clip that keeps every pixel.
fn surface(size: u32) -> (Pixmap, Mask) {
    let budget = usize::try_from(size * size).unwrap();
    let target = Pixmap::new(size, size, Rgba8::TRANSPARENT, budget).unwrap();
    let mut full = PathBuilder::new();
    full.rect(0.0, 0.0, f64::from(size), f64::from(size));
    let clip = mask(area(0, 0, size, size), &full.finish(), FillRule::NonZero);
    (target, clip)
}

fn fill(target: &mut Pixmap, clip: &Mask, path: Path, transform: Transform, paint: Rgba8) {
    Rasterizer::default()
        .fill_path(
            target,
            path,
            transform,
            FillRule::NonZero,
            &Paint::Solid(paint),
            clip,
        )
        .unwrap();
}

/// Strokes `path` with `style` in black onto `target`, clipped to `clip`.
fn stroke(
    target: &mut Pixmap,
    clip: &Mask,
    path: &Path,
    transform: Transform,
    style: &Stroke,
) -> Result<()> {
    let (outline, device) = stroke_outline(path, transform, style, &Limits::default())?;
    fill(target, clip, outline, device, Rgba8::BLACK);
    Ok(())
}

/// The alpha channel of every pixel.
fn alpha(target: &Pixmap) -> Vec<u8> {
    target
        .data()
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| p[3])
        .collect()
}

#[test]
fn clipping_and_premultiplied_composition() {
    let path = Path::rect(Rect::from_corners(0.0, 0.0, 1.0, 1.0).unwrap());
    let mut clip_path = PathBuilder::new();
    clip_path.rect(0.0, 0.0, 0.5, 1.0);
    let mut clip = mask(area(0, 0, 1, 1), &clip_path.finish(), FillRule::NonZero);
    let full = mask(area(0, 0, 1, 1), &path, FillRule::NonZero);
    clip.intersect(&full).unwrap();
    assert_eq!(clip.coverage(), &[128]);
    let mut target = Pixmap::new(1, 1, Rgba8::TRANSPARENT, 1).unwrap();
    fill(
        &mut target,
        &clip,
        path,
        Transform::IDENTITY,
        Rgba8::new(255, 0, 0, 128),
    );
    assert_eq!(target.data(), &[64, 0, 0, 64]);
}

#[test]
fn stroke_transform_hairline_dash_and_miter() {
    let mut builder = PathBuilder::new();
    builder.polyline([Point::new(1.0, 2.0), Point::new(3.0, 2.0)]);
    let path = builder.finish();
    let transform = Transform::scale(2.0, 1.0);
    let (mut target, clip) = surface(8);
    let square = Stroke {
        width: 2.0,
        cap: LineCap::ProjectingSquare,
        ..Stroke::default()
    };
    stroke(&mut target, &clip, &path, transform, &square).unwrap();
    // User-space square cap extends 1 unit, hence 2 device pixels in x.
    assert_eq!(alpha(&target)[8..16], [255; 8]);
    let (mut target, clip) = surface(8);
    let hairline = Stroke {
        width: 0.0,
        ..Stroke::default()
    };
    stroke(&mut target, &clip, &path, transform, &hairline).unwrap();
    assert_eq!(
        alpha(&target).iter().map(|&a| u32::from(a)).sum::<u32>(),
        4 * 256
    );
    let dashed = Stroke {
        width: 2.0,
        dash_array: vec![1.0, 1.0],
        ..Stroke::default()
    };
    let (outline, _) =
        stroke_outline(&path, Transform::IDENTITY, &dashed, &Limits::default()).unwrap();
    let dashes = mask(area(0, 0, 8, 8), &outline, FillRule::NonZero);
    assert_eq!(
        dashes.coverage().iter().map(|&n| u32::from(n)).sum::<u32>(),
        2 * 255
    );
    for join in [LineJoin::Miter, LineJoin::Bevel, LineJoin::Round] {
        let mut p = PathBuilder::new();
        p.polyline([
            Point::new(1.0, 6.0),
            Point::new(4.0, 2.0),
            Point::new(7.0, 6.0),
        ]);
        let (outline, _) = stroke_outline(
            &p.finish(),
            Transform::IDENTITY,
            &Stroke {
                join,
                ..Stroke::default()
            },
            &Limits::default(),
        )
        .unwrap();
        assert!(!outline.is_empty());
    }
}

#[test]
fn invalid_geometry_and_budgets_return_errors() {
    let mut builder = PathBuilder::new();
    builder.move_to(Point::new(f64::NAN, 0.0));
    assert_eq!(
        Mask::from_path(
            area(0, 0, 1, 1),
            builder.finish(),
            Transform::IDENTITY,
            FillRule::NonZero,
            pixels(1)
        )
        .unwrap_err()
        .kind,
        ErrorKind::InvalidArgument
    );
    assert_eq!(
        Mask::from_path(
            area(0, 0, 8, 8),
            Path::default(),
            Transform::IDENTITY,
            FillRule::NonZero,
            pixels(63)
        )
        .unwrap_err()
        .kind,
        ErrorKind::Limit
    );
    let zero_dashes = Stroke {
        dash_array: vec![0.0],
        ..Stroke::default()
    };
    assert_eq!(
        stroke_outline(
            &Path::default(),
            Transform::IDENTITY,
            &zero_dashes,
            &Limits::default()
        )
        .unwrap_err()
        .kind,
        ErrorKind::InvalidArgument
    );
}

#[test]
fn mask_intersection_uses_geometry_and_is_atomic() {
    let mut builder = PathBuilder::new();
    builder.rect(0.0, 0.0, 0.5, 1.0);
    let left = mask(area(0, 0, 1, 1), &builder.finish(), FillRule::NonZero);
    let mut intersection = left.clone();
    intersection.intersect(&left).unwrap();
    assert_eq!(intersection.coverage(), &[128]);
    builder.rect(0.5, 0.0, 0.5, 1.0);
    let right = mask(area(0, 0, 1, 1), &builder.finish(), FillRule::NonZero);
    intersection.intersect(&right).unwrap();
    assert_eq!(intersection.coverage(), &[0]);
    let mut current = left;
    for _ in 0..8 {
        let other = current.clone();
        current.intersect(&other).unwrap();
    }
    let previous = current.clone();
    assert_eq!(
        current.intersect(&previous).unwrap_err().kind,
        ErrorKind::Limit
    );
    assert_eq!(current, previous);
}

#[test]
fn large_compound_triangles_and_caller_budgets() {
    let mut builder = PathBuilder::new();
    for y in 0..200 {
        for x in 0..250 {
            let (x, y) = (f64::from(x * 2), f64::from(y * 2));
            builder.polygon([
                Point::new(x, y),
                Point::new(x + 1.0, y),
                Point::new(x, y + 1.0),
            ]);
        }
    }
    let path = builder.finish();
    let triangles = mask(area(0, 0, 500, 400), &path, FillRule::NonZero);
    for (y, row) in triangles.coverage().as_chunks::<500>().0.iter().enumerate() {
        for (x, &value) in row.iter().enumerate() {
            assert_eq!(value, if x % 2 == 0 && y % 2 == 0 { 128 } else { 0 });
        }
    }
    for limits in [
        Limits {
            path_segments: 10,
            ..Limits::default()
        },
        Limits {
            raster_work: 0,
            ..Limits::default()
        },
        Limits {
            mask_sources: 0,
            ..Limits::default()
        },
    ] {
        assert_eq!(
            Mask::from_path(
                area(0, 0, 500, 400),
                path.clone(),
                Transform::IDENTITY,
                FillRule::NonZero,
                limits,
            )
            .unwrap_err()
            .kind,
            ErrorKind::Limit
        );
    }
}

#[test]
fn even_odd_and_nonzero_hold_for_both_winding_signs() {
    // A whole-pixel square inset by `inset` from a 6 × 6 area, in either
    // orientation; nested squares wind ±1, ±2 and ±3 from the outside in.
    let square = |inset: f64, reverse: bool| {
        let (near, far) = (inset, 6.0 - inset);
        let mut corners = vec![
            Point::new(near, near),
            Point::new(far, near),
            Point::new(far, far),
            Point::new(near, far),
        ];
        if reverse {
            corners.reverse();
        }
        corners
    };
    let ring = |i: usize| {
        let (x, y) = (i % 6, i / 6);
        x.min(y).min(5 - x).min(5 - y)
    };
    for reverse in [false, true] {
        let mut builder = PathBuilder::new();
        for inset in [0.0, 1.0, 2.0] {
            builder.polygon(square(inset, reverse));
        }
        let nested = builder.finish();
        let even_odd = mask(area(0, 0, 6, 6), &nested, FillRule::EvenOdd);
        let nonzero = mask(area(0, 0, 6, 6), &nested, FillRule::NonZero);
        let pairs = even_odd.coverage().iter().zip(nonzero.coverage());
        for (i, (&odd, &nonzero)) in pairs.enumerate() {
            let expected = if ring(i) == 1 { 0 } else { 255 };
            assert_eq!((odd, nonzero), (expected, 255), "{reverse} {i}");
        }
    }
    // Opposite orientations cancel: the center is a hole under both rules.
    let mut builder = PathBuilder::new();
    builder.polygon(square(0.0, false));
    builder.polygon(square(2.0, true));
    let hole = builder.finish();
    for rule in [FillRule::EvenOdd, FillRule::NonZero] {
        for (i, &value) in mask(area(0, 0, 6, 6), &hole, rule)
            .coverage()
            .iter()
            .enumerate()
        {
            let expected = if ring(i) == 2 { 0 } else { 255 };
            assert_eq!(value, expected, "{rule:?} {i}");
        }
    }
}

#[test]
fn masks_share_the_callers_path() {
    let path = Arc::new(Path::rect(
        Rect::from_corners(0.25, 0.25, 1.75, 1.75).unwrap(),
    ));
    let limits = Limits::default();
    let whole = Mask::from_path(
        area(0, 0, 2, 2),
        Arc::clone(&path),
        Transform::IDENTITY,
        FillRule::NonZero,
        limits,
    )
    .unwrap();
    assert_eq!(Arc::strong_count(&path), 2);
    // Each pixel holds 3/4 × 3/4 of the rectangle: 0.5625 × 255 = 143.4.
    assert_eq!(whole.coverage(), [143; 4]);
    let clipped = whole
        .clipped_path(
            Arc::clone(&path),
            Transform::IDENTITY,
            FillRule::NonZero,
            limits,
        )
        .unwrap()
        .unwrap();
    // The clipped mask holds the path and the clip's shape, both shared.
    assert_eq!(Arc::strong_count(&path), 4);
    assert_eq!(clipped.coverage(), whole.coverage());
}
