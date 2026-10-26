//! Repeated strokes give the bytes of their own preparation.

use super::*;
use crate::testing::{raster, window};
use crate::{Area, LineCap, LineJoin, PathBuilder};
use pagura_core::Limits;

fn options(limits: &Limits) -> ShapeOptions<'_> {
    ShapeOptions {
        area: Area {
            region: window(64, 48),
            band_height: 16,
        },
        limits,
    }
}

/// Each placement's bytes through `templates` and through its own
/// preparation, and whether it reused a template: a reused shape has no
/// resolved rows where its own one has some.
fn place(
    (path, stroke): (&Path, &Stroke),
    (x, y): (f64, f64),
    options: &ShapeOptions<'_>,
    templates: &mut StrokeTemplates,
) -> (Vec<u8>, Vec<u8>, bool) {
    let transform = Transform::new(1.0, 0.0, 0.0, 1.0, x, y);
    let own = Shape::stroke(
        path,
        stroke,
        Transform::IDENTITY,
        transform,
        Adjustment::None,
        options,
    )
    .unwrap();
    let repeated = Shape::stroke_repeated(
        path,
        stroke,
        (Transform::IDENTITY, transform),
        Adjustment::None,
        None,
        options,
        templates,
    )
    .unwrap();
    assert!(
        own.as_ref().is_some_and(|s| !s.resolved.is_empty()),
        "the stroke needs the resolver"
    );
    let reused = repeated.as_ref().is_some_and(|s| s.resolved.is_empty());
    let bytes = |s: Option<&Shape>| raster(&options.area, s, None).0;
    (bytes(repeated.as_ref()), bytes(own.as_ref()), reused)
}

/// A loop whose thick round-joined stroke overlaps itself.
fn curl() -> Path {
    let mut b = PathBuilder::new();
    b.move_to(Point::new(0.0, 0.0));
    b.cubic_to(
        Point::new(9.0, -5.0),
        Point::new(13.0, 11.0),
        Point::new(5.0, 12.0),
    )
    .unwrap();
    b.cubic_to(
        Point::new(-2.0, 13.0),
        Point::new(1.0, 4.0),
        Point::new(6.0, 5.0),
    )
    .unwrap();
    b.close();
    b.finish()
}

#[test]
fn repeated_strokes_give_the_bytes_of_their_own_preparation() {
    let limits = Limits::default();
    let options = options(&limits);
    let stroke = Stroke {
        width: 2.4,
        join: LineJoin::Round,
        cap: LineCap::Round,
        ..Stroke::default()
    };
    let path = curl();
    let mut templates = StrokeTemplates::default();
    let mut reused = 0;
    for (n, origin) in [
        (3.0, 4.0),
        (22.25, 4.5),
        (5.125, 27.0),
        (37.3, 21.7),
        (19.0625, 15.33),
        (44.5, 3.0),
        (31.17, 30.91),
    ]
    .into_iter()
    .enumerate()
    {
        let (repeated, own, hit) = place((&path, &stroke), origin, &options, &mut templates);
        assert_eq!(repeated, own, "placement {n} at {origin:?}");
        reused += usize::from(hit);
    }
    // The first two placements prepare their own shapes; the rest reuse.
    assert!(reused >= 4, "{reused} placements reused");
    assert!(
        templates.retained.as_ref().is_some_and(|m| m.used() > 0),
        "templates are charged"
    );
}

/// A notch narrower than the pen, with sides on whole pixels: half a pen
/// from them, the stroke's edges halve their pixels exactly.
fn notch() -> Path {
    let mut b = PathBuilder::new();
    b.polygon(
        [
            (0.0, 0.0),
            (8.0, 0.0),
            (8.0, 6.0),
            (5.0, 6.0),
            (5.0, 2.0),
            (4.25, 2.0),
            (4.25, 6.0),
            (0.0, 6.0),
        ]
        .map(|(x, y)| Point::new(x, y)),
    );
    b.finish()
}

#[test]
fn exact_half_coverage_is_never_certified() {
    let limits = Limits::default();
    let options = options(&limits);
    let stroke = Stroke {
        width: 1.0,
        join: LineJoin::Miter,
        cap: LineCap::Butt,
        ..Stroke::default()
    };
    let path = notch();
    let mut templates = StrokeTemplates::default();
    // On whole pixels a half-covered pixel is an exact rounding tie: no
    // error bound certifies it, so those placements prepare their own.
    for origin in [(2.0, 3.0), (20.0, 3.0), (2.0, 20.0), (30.0, 25.0)] {
        let (repeated, own, hit) = place((&path, &stroke), origin, &options, &mut templates);
        assert_eq!(repeated, own, "at {origin:?}");
        assert!(!hit, "a tie was certified at {origin:?}");
    }
    // Off the pixel grid, and off the decimal fractions whose coverage is
    // an odd multiple of 1/510, the ties are gone and the template serves.
    let mut reused = 0;
    for origin in [(40.371, 4.713), (12.613, 33.137), (45.151, 30.357)] {
        let (repeated, own, hit) = place((&path, &stroke), origin, &options, &mut templates);
        assert_eq!(repeated, own, "at {origin:?}");
        reused += usize::from(hit);
    }
    assert!(reused >= 2, "{reused} placements reused");
}
