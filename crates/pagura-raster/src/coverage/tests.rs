use super::*;

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
            quantize(area + cover)
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
            row.maximum = Limits::default().raster_work;
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
    row.maximum = Limits::default().raster_work;
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
    row.maximum = Limits::default().raster_work;
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
    row.maximum = Limits::default().raster_work;
    row.edge(edge(256.25, 256.25), 0.0, 1.0, 1).unwrap();
    let cells: usize = row.touched.iter().map(|w| w.count_ones() as usize).sum();
    let cost = cells + 2 * row.touched.len() + 1;
    row.maximum = row.work + cost - 1;
    assert_eq!(
        row.finish(&mut [0; 257]).unwrap_err().kind,
        pagura_core::ErrorKind::Limit
    );
    row.maximum += 1;
    let previous = row.work;
    compare_dense(&mut row);
    assert_eq!(row.work - previous, cost);
    row.maximum = Limits::default().raster_work;
    row.edge(edge(256.5, 256.5), 0.0, 1.0, 1).unwrap();
    row.reset(1, 17).unwrap();
    row.maximum = Limits::default().raster_work;
    assert_eq!(compare_dense(&mut row), [0]);
}
