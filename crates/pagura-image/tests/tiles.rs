#![forbid(unsafe_code)]
//! Explicit plural surfaces retain original admission and complete-data rules.

use pagura_core::{ErrorKind, Limits, ObjectId, Recovery, TiledImageLimits};
use pagura_document::{Document, OpenOptions};
use pagura_image::{DecodeRequest, ImageSource, Layout, Orientation, Reduce, SampleRect};
use pagura_testkit::fixture;

const MASK: ObjectId = ObjectId::new(6, 0);

fn original_limits() -> Limits {
    Limits {
        pixels: 10,
        decoded_bytes: 65_536,
        decode_bytes: 65_536,
        tiled_images: TiledImageLimits {
            samples: 70,
            work: 1_000,
        },
        ..Limits::default()
    }
}

fn source_with_limits(limits: &Limits, recovery: Recovery) -> pagura_core::Result<ImageSource> {
    let document = Document::open(
        fixture("packed_mask_tiles", "small-flate.pdf"),
        &OpenOptions {
            limits: *limits,
            recovery,
            ..OpenOptions::default()
        },
    )?;
    let object = document.object(MASK)?;
    let ordinary = ImageSource::from_xobject(&document, object);
    assert!(matches!(ordinary, Err(ref error) if error.kind == ErrorKind::Limit));
    let source = ImageSource::from_xobject_with_tiling(&document, object)?;
    assert_eq!(
        document.diagnostics(),
        Vec::<pagura_core::Diagnostic>::new()
    );
    Ok(source)
}

#[test]
fn deferred_sources_outlive_the_document_and_never_escape_whole_decode_limits() {
    let original = original_limits();
    for recovery in [Recovery::Strict, Recovery::Repair] {
        let source = source_with_limits(&original, recovery).unwrap();
        assert!(source.is_tiled_mask());
        assert_eq!((source.width(), source.height()), (10, 7));
        let raised = Limits {
            pixels: usize::MAX,
            ..Limits::default()
        };
        for layout in [Layout::Interleaved, Layout::Native] {
            for region in [
                None,
                Some(SampleRect {
                    x0: 0,
                    y0: 0,
                    x1: 1,
                    y1: 1,
                }),
            ] {
                let request = DecodeRequest::new(Reduce::Full, region, layout);
                let error = source.decode(&request, &raised, recovery).unwrap_err();
                assert_eq!(error.kind, ErrorKind::Limit);
                assert_eq!(error.object, Some(MASK));
            }
        }
        let layout = source.tile_layout(Orientation::Rows, &raised).unwrap();
        assert_eq!(layout.height(), 1);
        assert!(layout.retained_bytes() <= layout.new_peak_bytes());
        let tiled = source.decode_tiles(Orientation::Rows, &raised).unwrap();
        assert!(tiled.retained_bytes() <= layout.retained_bytes());
        assert_eq!(source.tile_height(Orientation::Rows, &raised).unwrap(), 1);
        assert_eq!(tiled.tiles().len(), 7);
        assert!(tiled.tiles().iter().all(|tile| tile.image().height == 1));
        let rows: Vec<u8> = (0..tiled.height())
            .flat_map(|y| tiled.row(y).unwrap().iter().copied())
            .collect();
        assert_eq!(rows, fixture("packed_mask_tiles", "bits-rectangle-pad6.in"));
        let transposed = source
            .decode_tiles(Orientation::Transposed, &raised)
            .unwrap();
        let rows: Vec<u8> = (0..transposed.height())
            .flat_map(|y| transposed.row(y).unwrap().iter().copied())
            .collect();
        assert_eq!(
            rows,
            fixture("packed_mask_tiles", "bits-rectangle-pad6.transposed")
        );
    }
}

#[test]
fn failed_lower_requests_cannot_poison_later_success_or_raise_original_limits() {
    let source = source_with_limits(&original_limits(), Recovery::Repair).unwrap();
    let retained = source.retained_bytes();
    let allowed = Limits {
        pixels: 20,
        ..Limits::default()
    };
    let lower = [
        Limits {
            pixels: 9,
            ..allowed
        },
        Limits {
            tiled_images: TiledImageLimits {
                samples: 69,
                ..allowed.tiled_images
            },
            ..allowed
        },
        Limits {
            tiled_images: TiledImageLimits {
                work: 1,
                ..allowed.tiled_images
            },
            ..allowed
        },
        Limits {
            decode_bytes: retained - 1,
            ..allowed
        },
        Limits {
            stream_bytes: 1,
            ..allowed
        },
    ];
    for refused in lower {
        for sequence in [[refused, allowed], [allowed, refused]] {
            for limits in sequence {
                let height = source.tile_height(Orientation::Rows, &limits);
                let result = source.decode_tiles(Orientation::Rows, &limits);
                if limits == allowed {
                    assert_eq!(height.unwrap(), 1);
                    let tiles = result.unwrap();
                    assert_eq!(tiles.tiles().len(), 7);
                    assert!(
                        tiles
                            .tiles()
                            .iter()
                            .all(|tile| tile.image().width == 10 && tile.image().height == 1)
                    );
                } else {
                    assert_eq!(height.unwrap_err().kind, ErrorKind::Limit);
                    let error = result.unwrap_err();
                    assert_eq!(error.kind, ErrorKind::Limit);
                    assert_eq!(error.object, Some(MASK));
                }
                assert_eq!(source.retained_bytes(), retained);
            }
        }
    }
}

#[test]
fn ordinary_bounded_sources_keep_their_original_whole_image_route() {
    let document = Document::open(
        fixture("packed_mask_tiles", "small-flate.pdf"),
        &OpenOptions::default(),
    )
    .unwrap();
    let source =
        ImageSource::from_xobject_with_tiling(&document, document.object(MASK).unwrap()).unwrap();
    assert!(!source.is_tiled_mask());
    assert_eq!(
        source
            .tile_height(Orientation::Rows, &document.limits())
            .unwrap_err()
            .kind,
        ErrorKind::InvalidArgument
    );
    let decoded = source
        .decode(&DecodeRequest::FULL, &document.limits(), Recovery::Strict)
        .unwrap();
    assert_eq!((decoded.image.width, decoded.image.height), (10, 7));
    assert_eq!(
        source
            .decode_tiles(Orientation::Rows, &document.limits())
            .unwrap_err()
            .kind,
        ErrorKind::InvalidArgument
    );
}

#[test]
fn validated_cache_shapes_retile_for_lower_valid_pixel_requests() {
    let original = Limits {
        pixels: 30,
        ..original_limits()
    };
    let source = source_with_limits(&original, Recovery::Strict).unwrap();
    let retained = source.retained_bytes();
    for orientation in [Orientation::Rows, Orientation::Transposed] {
        let expected = fixture(
            "packed_mask_tiles",
            if orientation == Orientation::Rows {
                "bits-rectangle-pad6.in"
            } else {
                "bits-rectangle-pad6.transposed"
            },
        );
        let width = if orientation == Orientation::Rows {
            10
        } else {
            7
        };
        for pixels in [30, 20, 10, 6, 20, 30] {
            let requested = Limits {
                pixels,
                ..Limits::default()
            };
            let height = source.tile_height(orientation, &requested);
            if pixels < width {
                assert_eq!(height.unwrap_err().kind, ErrorKind::Limit);
                assert_eq!(
                    source
                        .decode_tiles(orientation, &requested)
                        .unwrap_err()
                        .kind,
                    ErrorKind::Limit
                );
            } else {
                assert_eq!(
                    usize::try_from(height.unwrap()).unwrap(),
                    pixels.min(original.pixels) / width
                );
                let decoded = source.decode_tiles(orientation, &requested).unwrap();
                assert!(decoded.tiles().iter().all(|tile| {
                    u64::from(tile.image().width) * u64::from(tile.image().height)
                        <= u64::try_from(pixels.min(original.pixels)).unwrap()
                }));
                let packed: Vec<u8> = (0..decoded.height())
                    .flat_map(|y| decoded.row(y).unwrap().iter().copied())
                    .collect();
                assert_eq!(packed, expected);
            }
            assert_eq!(source.retained_bytes(), retained);
        }
    }
}

#[test]
fn source_completeness_does_not_join_filter_or_sample_padding_repairs() {
    for recovery in [Recovery::Strict, Recovery::Repair] {
        for name in [
            "flate-short-checksum",
            "flate-checksum-mismatch",
            "flate-trailing-byte",
            "flate-short-rows",
            "flate-excess-rows",
        ] {
            let document = Document::open(
                fixture("packed_mask_tiles", &format!("{name}.pdf")),
                &OpenOptions {
                    recovery,
                    ..OpenOptions::default()
                },
            )
            .unwrap();
            let object = document.object(MASK).unwrap();
            let before = document.diagnostics();
            let error = ImageSource::from_xobject_with_tiling(&document, object).unwrap_err();
            assert_eq!(
                error.kind,
                ErrorKind::Syntax,
                "{name} {recovery:?}: {error:?}"
            );
            assert_eq!(error.object, Some(MASK));
            assert_eq!(document.diagnostics(), before);
        }
    }
}
