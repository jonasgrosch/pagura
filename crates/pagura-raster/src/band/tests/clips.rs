//! Clip preparation: disjoint and containing clips.

use super::*;

#[test]
fn disjoint_clips_remove_and_containing_clips_drop_out() {
    let area = area(0, 0, 16, 16, 4);
    let mark = polygon(&[
        Point::new(5.2, 5.1),
        Point::new(9.7, 6.3),
        Point::new(6.1, 9.9),
    ]);
    let mut away = PathBuilder::new();
    away.rect(12.0, 12.0, 3.0, 3.0);
    let away = away.finish();
    let limits = Limits::default();
    let clip = ClipShape::new(
        &[(&away, FillRule::NonZero)],
        Transform::IDENTITY,
        &public(area, &limits),
    )
    .unwrap();
    let shape = Shape::fill_in(
        &mark,
        Transform::IDENTITY,
        FillRule::NonZero,
        &options(area, &limits, Some(&clip)),
    )
    .unwrap();
    assert!(shape.is_none());
    // Clips that keep every pixel the mark can touch are dropped for it: no
    // edge clipping, masking or resolution, the work and bytes of the mark
    // alone.
    let wide = polygon(&[
        Point::new(0.5, 0.5),
        Point::new(15.5, 0.5),
        Point::new(8.0, 30.0),
    ]);
    let mut frame = PathBuilder::new();
    frame.rect(0.25, 0.75, 15.0, 14.5);
    let frame = frame.finish();
    let alone = Shape::fill_in(
        &mark,
        Transform::IDENTITY,
        FillRule::NonZero,
        &options(area, &limits, None),
    )
    .unwrap()
    .unwrap();
    let expected = raster(&area, Some(&alone), None).0;
    for clips in [
        vec![(&wide, FillRule::NonZero)],
        vec![(&frame, FillRule::NonZero)],
        vec![(&frame, FillRule::NonZero), (&wide, FillRule::EvenOdd)],
    ] {
        let clip = ClipShape::new(&clips, Transform::IDENTITY, &public(area, &limits)).unwrap();
        let clipped = Shape::fill_in(
            &mark,
            Transform::IDENTITY,
            FillRule::NonZero,
            &options(area, &limits, Some(&clip)),
        )
        .unwrap()
        .unwrap();
        assert!(!clipped.masked);
        assert_eq!(clipped.work(), alone.work());
        assert_eq!(raster(&area, Some(&clipped), Some(&clip)).0, expected);
    }
}

#[test]
fn clip_preparation_cells_and_rows_use_the_remaining_byte_budget() {
    let area = area(0, 0, 64, 64, 16);
    let mut triangle = PathBuilder::new();
    triangle.polygon([
        Point::new(0.25, 0.25),
        Point::new(63.75, 0.75),
        Point::new(32.25, 63.75),
    ]);
    let triangle = triangle.finish();
    let generous = Limits::default();
    assert!(
        ClipShape::new(
            &[(&triangle, FillRule::NonZero)],
            Transform::IDENTITY,
            &public(area, &generous)
        )
        .is_ok()
    );
    let limited = Limits {
        render_bytes: 4096,
        ..generous
    };
    let error = ClipShape::new(
        &[(&triangle, FillRule::NonZero)],
        Transform::IDENTITY,
        &public(area, &limited),
    )
    .unwrap_err();
    assert_eq!(error.message, RENDER_BYTES);
}

#[test]
fn a_nearly_flat_convex_side_keeps_a_crossing_mark_within_its_hull() {
    // A square turned by about 0.06 degrees: its top side rises one pixel
    // over a thousand. The box's part beside that side moves onto it, which
    // would reach hundreds of columns left of the box without the hull.
    let area = area(0, 0, 1024, 16, 16);
    let limits = Limits::default();
    let mut square = PathBuilder::new();
    square.polygon([
        Point::new(0.0, 0.0),
        Point::new(1000.0, 1.0),
        Point::new(999.0, 1001.0),
        Point::new(-1.0, 1000.0),
    ]);
    let square = square.finish();
    let clip = ClipShape::new(
        &[(&square, FillRule::NonZero)],
        Transform::IDENTITY,
        &public(area, &limits),
    )
    .unwrap();
    assert!(clip.paths().unwrap().convex().is_some());
    let mut mark = PathBuilder::new();
    mark.rect(500.0, -5.0, 10.0, 10.0);
    let mark = mark.finish();
    let shape = Shape::fill_in(
        &mark,
        Transform::IDENTITY,
        FillRule::NonZero,
        &options(area, &limits, Some(&clip)),
    )
    .unwrap()
    .unwrap();
    assert!(!shape.masked);
    let bounds = shape.bounds();
    assert!(bounds.x0 >= 500 && bounds.x1 <= 510, "{bounds:?}");
}

#[test]
fn a_ring_inside_a_round_clip_keeps_its_own_edges() {
    // A ring whose box corners lie outside a round clip, so that the clip's
    // boundary runs cross every one of its rows, though the ring itself
    // stays inside the clip.
    let area = area(0, 0, 32, 32, 8);
    let limits = Limits::default();
    let circle = |radius: f64, count: u32, reversed: bool| {
        let step = std::f64::consts::TAU / f64::from(count);
        (0..count)
            .map(|i| {
                let a = f64::from(i) * step * if reversed { -1.0 } else { 1.0 };
                Point::new(16.0 + radius * a.cos(), 16.0 + radius * a.sin())
            })
            .collect::<Vec<_>>()
    };
    let mut round = PathBuilder::new();
    round.polygon(circle(14.0, 160, false));
    let round = round.finish();
    let mut ring = PathBuilder::new();
    ring.polygon(circle(13.0, 96, false));
    ring.polygon(circle(9.0, 64, true));
    let ring = ring.finish();
    let clip = ClipShape::new(
        &[(&round, FillRule::NonZero)],
        Transform::IDENTITY,
        &public(area, &limits),
    )
    .unwrap();
    let prepare = |clip| {
        Shape::fill_in(
            &ring,
            Transform::IDENTITY,
            FillRule::NonZero,
            &options(area, &limits, clip),
        )
        .unwrap()
        .unwrap()
    };
    let alone = prepare(None);
    let clipped = prepare(Some(&clip));
    assert!(!clipped.masked);
    assert!(clipped.resolved.is_empty());
    assert_eq!(clipped.work(), alone.work());
    assert_eq!(
        raster(&area, Some(&clipped), Some(&clip)).0,
        raster(&area, Some(&alone), None).0
    );
}
