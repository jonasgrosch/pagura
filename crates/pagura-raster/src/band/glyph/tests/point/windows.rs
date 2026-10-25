//! Mixed bodies retain their work/range guarantees beyond one cell window.

use super::*;

const HEIGHTS: [u32; 5] = [1, 3, 16, 32, 256];

fn outline([width, height]: [u32; 2], point: Point, reversed: bool) -> Path {
    let (width, height) = (f64::from(width), f64::from(height));
    let mut path = PathBuilder::new();
    if reversed {
        path.move_to(Point::new(0.0, height));
        path.line_to(Point::new(width, height)).unwrap();
        path.line_to(Point::new(width, 0.0)).unwrap();
        path.line_to(Point::new(6.0, 0.0)).unwrap();
        path.cubic_to(Point::new(4.0, 4.0), Point::new(2.0, 4.0), Point::default())
            .unwrap();
    } else {
        path.move_to(Point::default());
        path.cubic_to(
            Point::new(2.0, 4.0),
            Point::new(4.0, 4.0),
            Point::new(6.0, 0.0),
        )
        .unwrap();
        path.line_to(Point::new(width, 0.0)).unwrap();
        path.line_to(Point::new(width, height)).unwrap();
        path.line_to(Point::new(0.0, height)).unwrap();
    }
    path.close();
    path.move_to(point);
    path.close();
    path.finish()
}

#[test]
fn tall_and_wide_mixed_bodies_bound_work_in_every_band_grid() {
    let limits = Limits::default();
    for dimensions @ [width, height] in [[12, 192], [6144, 80]] {
        for reversed in [false, true] {
            for point_x in [-1.5, 2.25] {
                let point = Point::new(point_x, 35.25);
                let edges = Arc::new(
                    GlyphEdges::new(
                        &outline(dimensions, point, reversed),
                        [1.0, 0.0, 0.0, 1.0],
                        &limits,
                    )
                    .unwrap(),
                );
                assert!(edges.relative().is_some());
                for first_row in [0, 48] {
                    let region = DeviceRect {
                        x0: 0,
                        y0: first_row,
                        x1: width + 8,
                        y1: height + 8,
                    };
                    let mut expected = None;
                    for band_height in HEIGHTS {
                        let options = ShapeOptions {
                            area: Area {
                                region,
                                band_height,
                            },
                            limits: &limits,
                            scratch: None,
                        };
                        let shape =
                            place(&edges, Point::new(3.137, 2.271), &Prep::new(&options, None))
                                .unwrap()
                                .unwrap();
                        let Body::Glyph(body) = &shape.body else {
                            panic!("mixed glyph stays shared");
                        };
                        assert_eq!(body.geometry, GlyphGeometry::Relative);
                        assert!(shape.resolved.is_empty());
                        let relative = edges.relative().unwrap();
                        assert_eq!(
                            shape.boundaries(),
                            relative.lines.len() + relative.curves.len() + 2
                        );
                        let scan_width = body.scan.1 - body.scan.0;
                        if width > 4096 {
                            assert!(scan_width > 4096);
                        }
                        let mut bytes = crate::testing::bytes();
                        let cells = crate::band::cells::Cells::new(scan_width, &mut bytes).unwrap();
                        assert!(shape.bounds().height() > cells.rows_fit(scan_width));
                        let (actual, work) =
                            crate::testing::raster(&options.area, Some(&shape), None);
                        assert!(work > 0 && work <= shape.work(), "{work}>{}", shape.work());
                        let signature =
                            (shape.bounds(), shape.work(), shape.retained_bytes(), actual);
                        if let Some(expected) = &expected {
                            assert_eq!(&signature, expected, "band {band_height}");
                        } else {
                            expected = Some(signature);
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn current_placed_range_errors_are_independent_of_bands_and_store_state() {
    let limits = Limits::default();
    let path = outline([12, 96], Point::new(2.25, 35.25), false);
    let build = || {
        let value = GlyphEdges::new(&path, [1.0, 0.0, 0.0, 1.0], &limits)?;
        Ok(Built {
            bytes: value.retained_bytes(),
            value,
        })
    };
    let store = Store::new(1 << 20);
    let warm = store.get_or_build(Key, build).unwrap();
    let cold = Arc::new(build().unwrap().value);
    assert!(cold.curved() && warm.curved());
    for (origin, in_range) in [
        (Point::new(crate::flatten::MAX_COORDINATE - 12.0, 0.0), true),
        (
            Point::new(crate::flatten::MAX_COORDINATE - 11.0, 0.0),
            false,
        ),
        (Point::new(f64::INFINITY, 0.0), false),
    ] {
        let mut expected = None;
        for band_height in HEIGHTS {
            let options = ShapeOptions {
                area: Area {
                    region: DeviceRect {
                        x0: 0,
                        y0: 0,
                        x1: 24,
                        y1: 112,
                    },
                    band_height,
                },
                limits: &limits,
                scratch: None,
            };
            for edges in [&cold, &warm] {
                let actual =
                    place(edges, origin, &Prep::new(&options, None)).map(|shape| shape.is_none());
                if in_range {
                    assert_eq!(actual, Ok(true));
                } else {
                    assert!(actual.is_err());
                }
                if let Some(expected) = &expected {
                    assert_eq!(&actual, expected);
                } else {
                    expected = Some(actual);
                }
            }
        }
    }
}

#[test]
fn a_point_left_of_the_output_region_keeps_the_full_scan_prefix() {
    let limits = Limits::default();
    for reversed in [false, true] {
        for point in [
            Point::new(-1.0, 35.25),
            Point::new(2.25, 35.25),
            Point::new(13.25, 35.25),
        ] {
            let edges = Arc::new(
                GlyphEdges::new(
                    &outline([12, 80], point, reversed),
                    [1.0, 0.0, 0.0, 1.0],
                    &limits,
                )
                .unwrap(),
            );
            for band_height in HEIGHTS {
                let full = ShapeOptions {
                    area: Area {
                        region: DeviceRect {
                            x0: 0,
                            y0: 0,
                            x1: 16,
                            y1: 88,
                        },
                        band_height,
                    },
                    limits: &limits,
                    scratch: None,
                };
                let origin = Point::new(0.0, 0.271);
                let shape = place(&edges, origin, &Prep::new(&full, None))
                    .unwrap()
                    .unwrap();
                let (pixels, spent) = crate::testing::raster(&full.area, Some(&shape), None);
                assert!(spent <= shape.work());
                for (x0, x1) in [(4, 16), (0, 10), (4, 10)] {
                    let cropped = ShapeOptions {
                        area: Area {
                            region: DeviceRect {
                                x0,
                                x1,
                                ..full.area.region
                            },
                            band_height,
                        },
                        limits: &limits,
                        scratch: None,
                    };
                    let shape = place(&edges, origin, &Prep::new(&cropped, None))
                        .unwrap()
                        .unwrap();
                    let (actual, spent) = crate::testing::raster(&cropped.area, Some(&shape), None);
                    assert!(spent <= shape.work());
                    let expected: Vec<_> = pixels
                        .as_chunks::<16>()
                        .0
                        .iter()
                        .flat_map(|row| &row[x0 as usize..x1 as usize])
                        .copied()
                        .collect();
                    assert_eq!(actual, expected, "{point:?}/{reversed}/band {band_height}");
                }
            }
        }
    }
}
