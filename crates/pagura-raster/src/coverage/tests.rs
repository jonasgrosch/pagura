use super::*;

/// The pixels `origin.0..origin.0 + width` × `origin.1..origin.1 + height`.
fn area(origin: (u32, u32), (width, height): (u32, u32)) -> IntRect {
    IntRect::new(origin.0, origin.1, width, height).unwrap()
}

fn edge(a: f64, b: f64) -> Edge {
    Edge {
        a: Point::new(a, 0.0),
        b: Point::new(b, 1.0),
        winding: 1,
        group: 0,
        rule: FillRule::NonZero,
        id: 0,
    }
}

// The original full-width finish is an independent reference for the sparse
// traversal. It also adds every zero delta and zero area explicitly.
fn compare_dense(row: &mut Row) -> Vec<u8> {
    let mut cover = 0.0;
    let expected: Vec<_> = row
        .areas
        .iter()
        .zip(&row.covers)
        .map(|(&area, &delta)| {
            cover += delta;
            quantize_unit(area + cover)
        })
        .collect();
    let mut actual = vec![0; expected.len()];
    row.finish(&mut actual).unwrap();
    assert_eq!(actual, expected);
    assert!(row.touched.iter().all(|&v| v == 0));
    assert!(row.areas.iter().all(|v| v.to_bits() == 0));
    assert!(row.covers.iter().all(|v| v.to_bits() == 0));
    actual
}

#[test]
fn sparse_finish_matches_dense_across_word_boundaries_and_reuse() {
    let mut row = Row::default();
    let mut seed = 0x63a9_b2c4_d67e_1234_u64;
    for width in [0, 1, 2, 63, 64, 65, 127, 128, 129, 4097, 7] {
        for _ in 0..20 {
            row.reset(width, 100).unwrap();
            row.work = Meter::new(Limits::default().raster_work, RASTER_WORK);
            for n in 0..32 {
                let mut coordinate = || {
                    seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                    let part = u32::try_from(seed >> 32).unwrap();
                    99.0 + f64::from(part) / f64::from(u32::MAX)
                        * f64::from(u32::try_from(width + 2).unwrap())
                };
                let e = edge(coordinate(), coordinate());
                let lo = f64::from(n % 4) / 4.0;
                row.edge(e, lo, lo + 0.25, if n % 3 == 0 { -1 } else { 1 })
                    .unwrap();
            }
            compare_dense(&mut row);
        }
    }
}

#[test]
fn prefix_cover_is_recorded_at_zero_even_for_an_outside_edge() {
    let mut row = Row::default();
    row.reset(129, 7).unwrap();
    row.work = Meter::new(Limits::default().raster_work, RASTER_WORK);
    row.edge(edge(200.0, 200.0), 0.0, 0.25, 1).unwrap();
    assert_eq!(row.touched, [1, 0, 0]);
    assert_eq!(compare_dense(&mut row), vec![64; 129]);
    row.edge(edge(71.25, 71.25), 0.0, 1.0, 1).unwrap();
    assert_eq!(row.touched, [1, 1, 0]);
    let actual = compare_dense(&mut row);
    assert_eq!(&actual[..64], &[255; 64]);
    assert_eq!(actual[64], 64);
    assert!(actual[65..].iter().all(|&v| v == 0));
}

#[test]
fn cancellation_and_quantization_thresholds_match_dense() {
    let mut row = Row::default();
    row.reset(193, 0).unwrap();
    row.work = Meter::new(Limits::default().raster_work, RASTER_WORK);
    for width in [0.0, 0.5 / 255.0, 0.5, 1.0 - 0.5 / 255.0, 1.0] {
        for x in [0.0, 63.0, 64.0, 128.0, 192.0] {
            for (lo, hi, sign) in [(0.0, 0.5, 1), (0.0, 0.5, -1), (0.0, 1.0, 1)] {
                row.edge(edge(x + width, x + width), lo, hi, sign).unwrap();
            }
        }
        compare_dense(&mut row);
    }
}

#[test]
fn finish_budget_failure_preserves_state_for_a_smaller_reset() {
    let mut row = Row::default();
    row.reset(257, 0).unwrap();
    row.work = Meter::new(Limits::default().raster_work, RASTER_WORK);
    row.edge(edge(256.25, 256.25), 0.0, 1.0, 1).unwrap();
    let cells: usize = row.touched.iter().map(|w| w.count_ones() as usize).sum();
    let cost = cells + 2 * row.touched.len() + 1;
    // One unit short: finish must fail before draining any state.
    row.work = Meter::new(cost - 1, RASTER_WORK);
    assert_eq!(
        row.finish(&mut [0; 257]).unwrap_err().kind,
        pagura_core::ErrorKind::Limit
    );
    row.work = Meter::new(cost, RASTER_WORK);
    compare_dense(&mut row);
    assert_eq!(row.work.used(), cost);
    row.work = Meter::new(Limits::default().raster_work, RASTER_WORK);
    row.edge(edge(256.5, 256.5), 0.0, 1.0, 1).unwrap();
    row.reset(1, 17).unwrap();
    row.work = Meter::new(Limits::default().raster_work, RASTER_WORK);
    assert_eq!(compare_dense(&mut row), [0]);
}

#[test]
fn many_subpixel_slivers_keep_their_combined_area() {
    // 8.5.3.3.2–3: disjoint interiors have the same union under either rule.
    // All coordinates are binary fractions. Each rectangle has area
    // (3/4) * 2^-21; 2^15 rectangles therefore cover 3/256 of one pixel.
    // Independent half-up rounding gives round(255 * 3/256) = 3. In
    // particular, discarding individually sub-grid contours is not harmless.
    let grid = f64::from(1_u32 << 20);
    for transpose in [false, true] {
        let mut builder = crate::PathBuilder::new();
        for i in 0..(1_u32 << 15) {
            let center = f64::from(4 * i + 2) / grid;
            let lo = center - 0.25 / grid;
            let thickness = 0.5 / grid;
            if transpose {
                builder.rect(lo, 0.125, thickness, 0.75);
            } else {
                builder.rect(0.125, lo, 0.75, thickness);
            }
        }
        let path = Arc::new(builder.finish());
        for rule in [FillRule::NonZero, FillRule::EvenOdd] {
            for origin in [(0, 0), (17, 29), (1 << 30, 1 << 30)] {
                let shapes = [Shape {
                    geometry: Geometry::Path(Arc::clone(&path)),
                    transform: Transform::translate(f64::from(origin.0), f64::from(origin.1)),
                    rule,
                }];
                let mut scratch = Scratch::default();
                let mut whole = [0; 6];
                coverage(
                    area(origin, (3, 2)),
                    &shapes,
                    &mut whole,
                    &Limits::default(),
                    &mut scratch,
                )
                .unwrap();
                assert_eq!(whole, [3, 0, 0, 0, 0, 0]);
                let mut accumulated = [0; 6];
                scratch
                    .accumulator()
                    .unwrap()
                    .render(
                        &shapes,
                        &mut accumulated,
                        origin,
                        (3, 2),
                        &Limits::default(),
                        0,
                    )
                    .unwrap();
                assert_eq!(accumulated, whole);
                let mut accumulated_tile = [0];
                scratch
                    .accumulator()
                    .unwrap()
                    .render(
                        &shapes,
                        &mut accumulated_tile,
                        origin,
                        (1, 1),
                        &Limits::default(),
                        0,
                    )
                    .unwrap();
                assert_eq!(accumulated_tile, [3]);
                for y in 0..2 {
                    for x in 0..3 {
                        let mut tile = [0];
                        coverage(
                            area((origin.0 + x, origin.1 + y), (1, 1)),
                            &shapes,
                            &mut tile,
                            &Limits::default(),
                            &mut scratch,
                        )
                        .unwrap();
                        assert_eq!(tile[0], whole[usize::try_from(y * 3 + x).unwrap()]);
                    }
                }
            }
        }
    }
}

#[test]
fn retained_storage_tracks_backend_switches_and_partial_errors() {
    let mut builder = crate::PathBuilder::new();
    builder.rect(0.0, 0.0, 8.0, 8.0);
    let rectangle = builder.finish();
    builder.move_to(Point::new(0.0, 0.0));
    builder
        .cubic_to(
            Point::new(8.0, 0.0),
            Point::new(8.0, 8.0),
            Point::new(0.0, 8.0),
        )
        .unwrap();
    builder.close();
    let curved = builder.finish();
    builder.rect(0.25, 0.25, 3.5, 3.5);
    builder.rect(2.25, 2.25, 3.5, 3.5);
    let components = builder.finish();
    let mut scratch = Scratch::default();
    let mut output = [0; 64];
    for path in [&rectangle, &curved, &components, &rectangle] {
        let shapes = [Shape {
            geometry: Geometry::Path(Arc::new(path.clone())),
            transform: Transform::IDENTITY,
            rule: FillRule::NonZero,
        }];
        coverage(
            area((0, 0), (8, 8)),
            &shapes,
            &mut output,
            &Limits::default(),
            &mut scratch,
        )
        .unwrap();
        assert_eq!(
            scratch.retained_bytes(),
            scratch.exact.retained_bytes()
                + capacity_bytes(&scratch.accumulation)
                + scratch
                    .accumulation
                    .iter()
                    .map(accumulation::Accumulator::retained_bytes)
                    .sum::<usize>()
        );
    }
    let mut scratch = Scratch::default();
    let shapes = [Shape {
        geometry: Geometry::Path(Arc::new(curved)),
        transform: Transform::IDENTITY,
        rule: FillRule::NonZero,
    }];
    let limits = Limits {
        raster_work: 16,
        ..Limits::default()
    };
    assert!(
        coverage(
            area((0, 0), (8, 8)),
            &shapes,
            &mut output,
            &limits,
            &mut scratch
        )
        .is_err()
    );
    assert!(scratch.retained_bytes() > 0);
    assert_eq!(
        scratch.retained_bytes(),
        scratch.exact.retained_bytes()
            + capacity_bytes(&scratch.accumulation)
            + scratch
                .accumulation
                .iter()
                .map(accumulation::Accumulator::retained_bytes)
                .sum::<usize>()
    );
}

/// Glyph polygons through both backends.
mod glyph {
    use super::*;
    use crate::{GlyphGeometry, PathBuilder};

    #[test]
    fn glyph_point_classification_and_trailing_contours_survive_both_backends() {
        let limits = Limits::default();
        let mut path = PathBuilder::new();
        path.move_to(Point::new(0.0, 0.0));
        path.line_to(Point::new(0.0, 1.0)).unwrap();
        path.close();
        path.move_to(Point::new(7.0, 7.0));
        // This singular linear map collapses the source controls, so the first
        // contour is a genuine point. The final open point still paints nothing.
        let geometry = Arc::new(
            GlyphGeometry::new(
                &path.finish(),
                Transform::new(1.0, 0.0, 0.0, 0.0, 0.0, 0.0),
                &limits,
                usize::MAX,
            )
            .unwrap(),
        );
        assert!(geometry.contours()[0].is_degenerate());
        for origin in [(0, 0), (17, 29), (1 << 30, 1 << 30)] {
            let shape = Shape {
                geometry: Geometry::Glyph(Arc::clone(&geometry)),
                transform: Transform::translate(
                    f64::from(origin.0) + 2.25,
                    f64::from(origin.1) + 3.75,
                ),
                rule: FillRule::NonZero,
            };
            for render in [coverage, coverage_accumulated] {
                let mut output = [0; 64];
                render(
                    area(origin, (8, 8)),
                    std::slice::from_ref(&shape),
                    &mut output,
                    &limits,
                    &mut Scratch::default(),
                )
                .unwrap();
                let mut expected = [0; 64];
                expected[3 * 8 + 2] = 255;
                assert_eq!(output, expected);
                let mut pixel = [0];
                render(
                    area((origin.0 + 2, origin.1 + 3), (1, 1)),
                    std::slice::from_ref(&shape),
                    &mut pixel,
                    &limits,
                    &mut Scratch::default(),
                )
                .unwrap();
                assert_eq!(pixel, [255]);
                let restricted = Limits {
                    path_segments: geometry.vertices() - 1,
                    ..limits
                };
                assert_eq!(
                    render(
                        area(origin, (8, 8)),
                        std::slice::from_ref(&shape),
                        &mut output,
                        &restricted,
                        &mut Scratch::default()
                    )
                    .unwrap_err()
                    .kind,
                    pagura_core::ErrorKind::Limit
                );
            }
        }
    }

    #[test]
    fn coincident_glyph_approximation_vertices_do_not_create_a_floor_cell() {
        let limits = Limits::default();
        let mut path = PathBuilder::new();
        let p = Point::new(4.0, 4.0);
        path.move_to(p);
        path.cubic_to(Point::new(4.0001, 4.0), Point::new(4.0, 4.0001), p)
            .unwrap();
        path.close();
        let glyph = Arc::new(
            GlyphGeometry::new(&path.finish(), Transform::IDENTITY, &limits, usize::MAX).unwrap(),
        );
        assert!(!glyph.contours()[0].is_degenerate());
        assert!(glyph.contours()[0].points().iter().all(|point| *point == p));
        let shapes = [Shape {
            geometry: Geometry::Glyph(glyph),
            transform: Transform::IDENTITY,
            rule: FillRule::NonZero,
        }];
        for render in [coverage, coverage_accumulated] {
            let mut output = [255; 64];
            render(
                area((0, 0), (8, 8)),
                &shapes,
                &mut output,
                &limits,
                &mut Scratch::default(),
            )
            .unwrap();
            assert_eq!(output, [0; 64]);
        }
    }
}
