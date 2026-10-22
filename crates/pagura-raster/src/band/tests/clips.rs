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
    // edge clipping, overlay or analysis, the work and bytes of the mark
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
        assert!(!clipped.overlay);
        assert_eq!(clipped.work(), alone.work());
        assert_eq!(raster(&area, Some(&clipped), Some(&clip)).0, expected);
    }
}
