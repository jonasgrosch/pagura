//! Clip preparation: disjoint and containing clips, and clips multiplied
//! into shapes prepared outside them.

use super::*;

fn random_polygon(rng: &mut Lcg, count: usize, steps: u32) -> Path {
    let points: Vec<_> = (0..count)
        .map(|_| Point::new(rng.grid(-1.0, 25.0, steps), rng.grid(-1.0, 21.0, steps)))
        .collect();
    polygon(&points)
}

fn rectangle(rng: &mut Lcg, steps: u32) -> Path {
    let mut path = PathBuilder::new();
    path.rect(
        rng.grid(-1.0, 14.0, steps),
        rng.grid(-1.0, 12.0, steps),
        rng.grid(2.0, 14.0, steps),
        rng.grid(2.0, 12.0, steps),
    );
    path.finish()
}

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
        &public(area, &limits, Precision::Exact),
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
        let clip = ClipShape::new(
            &clips,
            Transform::IDENTITY,
            &public(area, &limits, Precision::Exact),
        )
        .unwrap();
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

/// A shape prepared outside its mark's clip, as `MarkCoverage` allows, has
/// the clip multiplied in: within one level wherever the shape or the clip
/// is empty or full on a pixel, never outside the clip, and within its
/// work bound.
#[test]
fn shapes_prepared_outside_their_clip_multiply_it_in() {
    let mut rng = Lcg(4242);
    let limits = Limits::default();
    for case in 0..30 {
        let area = area(0, 0, 24, 20, [1, 5, 16][case % 3]);
        let steps = if case % 2 == 0 { 256 } else { 10 };
        let mark = random_polygon(&mut rng, 3 + case % 5, steps);
        let rect = rectangle(&mut rng, steps);
        let path = random_polygon(&mut rng, 3 + case % 3, steps);
        let clips: [Vec<(&Path, FillRule)>; 3] = [
            vec![(&rect, FillRule::NonZero)],
            vec![(&path, FillRule::NonZero)],
            vec![(&rect, FillRule::NonZero), (&path, FillRule::EvenOdd)],
        ];
        for clips in &clips {
            let options = public(area, &limits, Precision::Exact);
            let clip = ClipShape::new(clips, Transform::IDENTITY, &options).unwrap();
            let fill = |clip: Option<&ClipShape>| {
                Shape::fill_with_clip(
                    &mark,
                    Transform::IDENTITY,
                    FillRule::NonZero,
                    clip,
                    &options,
                )
                .unwrap()
            };
            let alone =
                fill(None).map_or_else(|| vec![0; 24 * 20], |s| raster(&area, Some(&s), None).0);
            let exact = fill(Some(&clip)).map_or_else(
                || vec![0; 24 * 20],
                |s| raster(&area, Some(&s), Some(&clip)).0,
            );
            let clip_alone = raster(&area, None, Some(&clip)).0;
            let Some(shape) = fill(None) else {
                continue;
            };
            let mark = MarkCoverage {
                shape: Some(&shape),
                clip: Some(&clip),
            };
            let mut bytes = Meter::new(usize::MAX, "bytes");
            let mut raster = BandRasterizer::new(area, &mut bytes).unwrap();
            let mut band = BandCoverage::with_capacity(Capacity::band(&area), &mut bytes).unwrap();
            let mut late = vec![0_u8; 24 * 20];
            for b in area.bands() {
                raster.rasterize(mark, b, &mut band);
                for row in band.rows() {
                    for span in row.spans() {
                        let (x, values) = match span {
                            Span::Full { x, len } => (x, vec![255; len as usize]),
                            Span::Partial { x, coverage } => (x, coverage.to_vec()),
                        };
                        let start = row.y as usize * 24 + x as usize;
                        late[start..start + values.len()].copy_from_slice(&values);
                    }
                }
            }
            assert!(raster.work_done() <= mark.work(&area));
            for i in 0..late.len() {
                if clip_alone[i] == 0 {
                    assert_eq!(late[i], 0, "outside the clip at {i}");
                }
                // A rectangle's factor rounds its own area: one level.
                if matches!(clip_alone[i], 0 | 255) || matches!(alone[i], 0 | 255) {
                    assert!(
                        late[i].abs_diff(exact[i]) <= 1,
                        "case {case} pixel {i}: {} vs {} (shape {}, clip {})",
                        late[i],
                        exact[i],
                        alone[i],
                        clip_alone[i]
                    );
                }
            }
        }
    }
}
