//! Full independent packed-byte oracles and aggregate admission boundaries.

use super::{DecodedTiles, ImageTile, InputSize, Plan};
use crate::source::header::{Codec, Header};
use crate::{ImageKind, Orientation, SourceMask};
use pagura_core::{ErrorKind, Limits, TiledImageLimits, Value};
use pagura_testkit::{Json, fixture, fixture_text};

struct Input {
    width: u32,
    height: u32,
    pixels: usize,
    bits: Vec<Vec<u16>>,
    packed: Vec<u8>,
    transposed: Vec<u8>,
}

impl Input {
    fn load(name: &str) -> Self {
        let params = Json::parse(&fixture_text(
            "packed_mask_tiles",
            &format!("bits-{name}.params"),
        ))
        .unwrap();
        let number = |key| params.get(key).and_then(Json::as_u64).unwrap();
        let bits = params
            .get("logical_bits")
            .and_then(Json::as_array)
            .unwrap()
            .iter()
            .map(|row| {
                row.as_array()
                    .unwrap()
                    .iter()
                    .map(|bit| u16::try_from(bit.as_u64().unwrap()).unwrap())
                    .collect()
            })
            .collect();
        Self {
            width: u32::try_from(number("width")).unwrap(),
            height: u32::try_from(number("height")).unwrap(),
            pixels: usize::try_from(number("custom_pixel_cap")).unwrap(),
            bits,
            packed: fixture("packed_mask_tiles", &format!("bits-{name}.in")),
            transposed: fixture("packed_mask_tiles", &format!("bits-{name}.transposed")),
        }
    }

    fn size(&self) -> InputSize {
        // A fixture caller reports the complete retained source separately
        // from its14payload bytes; the aggregate auxiliary gate must use it.
        InputSize {
            data: self.packed.len(),
            retained: 4_096,
        }
    }

    fn limits(&self) -> Limits {
        Limits {
            pixels: self.pixels,
            tiled_images: TiledImageLimits {
                samples: self.width * self.height,
                work: 1_000,
            },
            ..Limits::default()
        }
    }
}

fn header(width: u32, height: u32) -> Header {
    let kind = ImageKind::Color {
        color_space: Some(Value::name(b"DeviceGray")),
    };
    Header {
        width,
        height,
        declared_components: 1,
        declared_bits: 1,
        components: 1,
        bits: vec![1],
        initial: kind.clone(),
        kind,
        decode: Some(vec![0.0, 1.0]),
        interpolate: false,
        mask: SourceMask::None,
        intent: None,
        codec: Codec::None,
        stencil_after_decode: false,
    }
}

fn rows(image: &DecodedTiles) -> Vec<u8> {
    (0..image.height())
        .flat_map(|y| image.row(y).unwrap().iter().copied())
        .collect()
}

#[test]
fn logical_bit_oracles_exclude_padding_in_every_surface_and_transpose() {
    for name in [
        "rectangle-pad6",
        "last-logical-bit-pad7",
        "nonrectangular-pad3",
    ] {
        let input = Input::load(name);
        let header = header(input.width, input.height);
        let limits = input.limits();
        for orientation in [Orientation::Rows, Orientation::Transposed] {
            let plan = Plan::new(&header, input.size(), orientation, &limits).unwrap();
            let decoded = plan
                .decode(&header, &input.packed, orientation, &limits)
                .unwrap();
            let expected = if orientation == Orientation::Rows {
                &input.packed
            } else {
                &input.transposed
            };
            assert_eq!(rows(&decoded), *expected, "{name} {orientation:?}");
            let mut end = 0;
            for tile in decoded.tiles() {
                let bounds = tile.bounds();
                let image = tile.image();
                assert_eq!(bounds.y0, end);
                assert_eq!((bounds.x0, bounds.x1), (0, decoded.width()));
                assert_eq!(bounds.y1 - bounds.y0, image.height);
                assert!(
                    u64::from(image.width) * u64::from(image.height)
                        <= u64::try_from(limits.pixels).unwrap()
                );
                end = bounds.y1;
            }
            assert_eq!(end, decoded.height());
            for y in 0..decoded.height() {
                for x in 0..decoded.width() {
                    let expected = if orientation == Orientation::Rows {
                        input.bits[usize::try_from(y).unwrap()][usize::try_from(x).unwrap()]
                    } else {
                        input.bits[usize::try_from(x).unwrap()][usize::try_from(y).unwrap()]
                    };
                    assert_eq!(decoded.get(x, y), Some(expected));
                }
            }
            assert_eq!(decoded.get(decoded.width(), 0), None);
            assert_eq!(decoded.get(0, decoded.height()), None);
            assert_eq!(decoded.row(decoded.height()), None);
        }
    }
}

#[test]
fn sample_and_work_allowances_are_aggregate_and_checked_at_the_boundary() {
    let input = Input::load("rectangle-pad6");
    let header = header(input.width, input.height);
    for (orientation, work) in [(Orientation::Rows, 42), (Orientation::Transposed, 167)] {
        // Ordinary copy reads/writes14bytes, dispatches7rows and7tiles.
        // Transpose reads/writes70codes, zeroes10bytes, dispatches7rows/10tiles.
        let mut limits = input.limits();
        limits.tiled_images.work = work;
        assert!(Plan::new(&header, input.size(), orientation, &limits).is_ok());
        limits.tiled_images.work = work - 1;
        assert_eq!(
            Plan::new(&header, input.size(), orientation, &limits)
                .unwrap_err()
                .kind,
            ErrorKind::Limit
        );
        limits = input.limits();
        limits.tiled_images.samples -= 1;
        assert_eq!(
            Plan::new(&header, input.size(), orientation, &limits)
                .unwrap_err()
                .kind,
            ErrorKind::Limit
        );
    }
}

#[test]
fn output_accounts_for_every_surface_capacity_and_metadata() {
    let input = Input::load("rectangle-pad6");
    let header = header(input.width, input.height);
    let mut limits = input.limits();
    // Seven two-byte rows each reserve8packed bytes and4decode f64 slots;
    // the seven tile records and collection header are also retained.
    let expected =
        size_of::<DecodedTiles>() + 7 * (size_of::<ImageTile>() + 8 + 4 * size_of::<f64>());
    limits.decoded_bytes = expected;
    let decoded = Plan::new(&header, input.size(), Orientation::Rows, &limits)
        .unwrap()
        .decode(&header, &input.packed, Orientation::Rows, &limits)
        .unwrap();
    assert_eq!(decoded.retained_bytes(), expected);
    limits.decoded_bytes -= 1;
    assert_eq!(
        Plan::new(&header, input.size(), Orientation::Rows, &limits)
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
}

#[test]
fn auxiliary_source_rows_and_addressability_are_individually_bounded() {
    let input = Input::load("rectangle-pad6");
    let header = header(input.width, input.height);
    let mut limits = input.limits();
    limits.decode_bytes = input.size().retained - 1;
    assert_eq!(
        Plan::new(&header, input.size(), Orientation::Rows, &limits)
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
    limits = input.limits();
    limits.pixels = usize::try_from(input.width).unwrap() - 1;
    assert_eq!(
        Plan::new(&header, input.size(), Orientation::Rows, &limits)
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
    limits = input.limits();
    for bytes in [input.packed.len() - 1, input.packed.len() + 1] {
        assert_eq!(
            Plan::new(
                &header,
                InputSize {
                    data: bytes,
                    retained: 4_096
                },
                Orientation::Rows,
                &limits
            )
            .unwrap_err()
            .kind,
            ErrorKind::Syntax
        );
    }
    for (width, height) in [(0, 1), (1, 0), (u32::MAX, u32::MAX)] {
        let header = self::header(width, height);
        assert!(
            Plan::new(
                &header,
                InputSize {
                    data: 0,
                    retained: 0
                },
                Orientation::Rows,
                &limits
            )
            .is_err()
        );
    }
}

#[test]
fn original_source_limits_intersect_every_later_request_without_retuning_domains() {
    let original = Limits {
        pixels: 10,
        stream_bytes: 100,
        decoded_bytes: 65_536,
        decode_bytes: 65_536,
        tiled_images: TiledImageLimits {
            samples: 70,
            work: 1_000,
        },
        ..Limits::default()
    };
    let admission = super::Admission::new(&original, 17);
    let input = Input::load("rectangle-pad6");
    let header = header(input.width, input.height);
    let allowed = Limits {
        pixels: 20,
        tiled_images: TiledImageLimits {
            samples: 71,
            work: 1_001,
        },
        // These unrelated budgets keep their caller meaning.
        operations: 1,
        decode_work: 1,
        ..Limits::default()
    };
    for requested in [
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
    ] {
        for sequence in [[requested, allowed], [allowed, requested]] {
            for request in sequence {
                let effective = admission.limits_for(&request).unwrap();
                let result = Plan::new(&header, input.size(), Orientation::Rows, &effective);
                if request == allowed {
                    assert_eq!(effective.pixels, original.pixels);
                    assert_eq!(effective.tiled_images, original.tiled_images);
                    assert_eq!(effective.decode_work, 1);
                    assert_eq!(effective.operations, 1);
                    let decoded = result
                        .unwrap()
                        .decode(&header, &input.packed, Orientation::Rows, &effective)
                        .unwrap();
                    assert_eq!(rows(&decoded), input.packed);
                } else {
                    assert_eq!(result.unwrap_err().kind, ErrorKind::Limit);
                }
            }
        }
    }
    assert_eq!(
        admission
            .limits_for(&Limits {
                stream_bytes: 16,
                ..allowed
            })
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
    assert_eq!(
        admission
            .limits_for(&Limits::default())
            .unwrap()
            .stream_bytes,
        100
    );
}
