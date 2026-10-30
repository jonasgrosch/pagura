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
    origin: (f64, f64),
    options: &ShapeOptions<'_>,
    templates: &mut StrokeTemplates,
) -> (Vec<u8>, Vec<u8>, bool) {
    place_adjusted((path, stroke, Adjustment::None), origin, options, templates)
}

/// [`place`] under `adjustment`.
fn place_adjusted(
    (path, stroke, adjustment): (&Path, &Stroke, Adjustment),
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
        adjustment,
        options,
    )
    .unwrap();
    let repeated = Shape::stroke_repeated(
        path,
        stroke,
        (Transform::IDENTITY, transform),
        adjustment,
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
        templates.meter.as_ref().is_some_and(|m| m.used() > 0),
        "templates are charged"
    );
}

#[test]
fn adjusted_strokes_repeat_where_fitting_keeps_their_centrelines() {
    let limits = Limits::default();
    let options = options(&limits);
    // A thin stroke fitted to a one-pixel pen. The curl takes adjustment's
    // general route, which keeps its centreline; the notch is orthogonal
    // under an axis-aligned map, so adjustment snaps it to the grid.
    let thin = Stroke {
        width: 0.2,
        ..Stroke::default()
    };
    let origins = [
        (3.0, 4.0),
        (22.25, 4.5),
        (5.125, 27.0),
        (37.3, 21.7),
        (19.0625, 15.33),
        (44.5, 3.0),
        (31.17, 30.91),
    ];
    for (path, kept) in [(curl(), true), (notch(), false)] {
        let mut templates = StrokeTemplates::default();
        let mut reused = 0;
        for (n, origin) in origins.into_iter().enumerate() {
            let (repeated, own, hit) = place_adjusted(
                (&path, &thin, Adjustment::Adjusted),
                origin,
                &options,
                &mut templates,
            );
            assert_eq!(repeated, own, "placement {n} at {origin:?}");
            reused += usize::from(hit);
        }
        if kept {
            assert!(reused >= 4, "{reused} fitted placements reused");
        } else {
            assert_eq!(reused, 0, "snapped placements never repeat");
        }
    }
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

/// The bytes the cache keeps: its slots, its entries and what they hold.
fn kept_bytes(templates: &StrokeTemplates) -> usize {
    templates.table.slots.capacity() * size_of::<Slot>()
        + templates.entries.capacity() * size_of::<Entry>()
        + templates
            .entries
            .iter()
            .map(|e| e.key.len() * size_of::<u64>() + e.template.heap_bytes())
            .sum::<usize>()
}

#[test]
fn a_saturated_cache_admits_nothing_it_cannot_charge() {
    // An eighth of this budget cannot hold the slots of 300 strokes: the
    // table grows by doubling, with its old and new slots charged at once.
    let limits = Limits {
        render_bytes: 128 << 10,
        ..Limits::default()
    };
    let options = options(&limits);
    let path = curl();
    let stroke = |n: u32| Stroke {
        width: 2.0 + f64::from(n) / 1024.0,
        join: LineJoin::Round,
        cap: LineCap::Round,
        ..Stroke::default()
    };
    let mut templates = StrokeTemplates::default();
    let check = |templates: &StrokeTemplates| {
        let meter = templates.meter.as_ref().expect("the cache has a meter");
        assert!(meter.used() <= meter.max());
        assert_eq!(
            meter.used(),
            kept_bytes(templates),
            "every kept byte is charged, nothing else"
        );
    };
    // While the budget is free, a small outline keeps a template.
    let (notch, thin) = (
        notch(),
        Stroke {
            width: 1.0,
            join: LineJoin::Miter,
            cap: LineCap::Butt,
            ..Stroke::default()
        },
    );
    for origin in [(40.371, 4.713), (12.613, 33.137)] {
        let (repeated, own, _) = place((&notch, &thin), origin, &options, &mut templates);
        assert_eq!(repeated, own, "notch at {origin:?}");
        check(&templates);
    }
    assert_eq!(templates.entries.len(), 1, "the notch keeps a template");
    // Many strokes seen once fill the table to what the budget admits.
    for n in 0..300 {
        let (repeated, own, _) = place((&path, &stroke(n)), (3.25, 4.5), &options, &mut templates);
        assert_eq!(repeated, own, "stroke {n}");
        check(&templates);
    }
    assert!(
        templates.table.filled < 300,
        "{} strokes recorded",
        templates.table.filled
    );
    // Past saturation, strokes seen before, strokes never recorded and the
    // kept template all give their own bytes.
    for n in (0..8).chain(290..300) {
        for origin in [(22.25, 4.5), (5.125, 27.0), (37.3, 21.7)] {
            let (repeated, own, _) = place((&path, &stroke(n)), origin, &options, &mut templates);
            assert_eq!(repeated, own, "stroke {n} at {origin:?}");
            check(&templates);
        }
    }
    let (repeated, own, _) = place((&notch, &thin), (45.151, 30.357), &options, &mut templates);
    assert_eq!(repeated, own, "notch after saturation");
    check(&templates);
    // Seen again, the first strokes found no room for their templates.
    assert!(
        templates
            .table
            .slots
            .iter()
            .any(|s| matches!(s, Slot::Barren(_))),
        "a template was refused"
    );
}
