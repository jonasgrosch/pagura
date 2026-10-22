//! The sampler family against the exact general route it replaces.
//!
//! `oracle` is a port of the old exact route's formulas
//! (`pagura-render`'s `compositor/sampled.rs`, `compositor/reduced.rs` and
//! the texel code of `compositor/images.rs`), kept in their original
//! structure: per-pixel sampling with the streamed column caches, `[f64; 4]`
//! texels and the separate mask sampler. Both sides convert colours with the
//! same device-space double, so every colour, alpha and shape must agree
//! bit for bit.

use super::geometry::Geometry;
use super::texels::{Convert, Opacity, OpacityTexels, SampleTexels};
use super::{Description, ImageSampler, MaskDescription, Orientation, Paint, Prepared};
use crate::SpanColors;
use pagura_color::{ColorSpace, Conversion, Converter, Indexed, Painted, Process, Target};
use pagura_image::{Decoded, DecodedImage, ImageAlpha, ImageKind, MaskKind, Samples};
use pagura_raster::Transform;
use std::sync::Arc;

mod accuracy;
mod documents;
mod oracle;
mod timing;

/// A prepared conversion of `base` into a composite target of `channels`
/// process channels (1 Gray, 3 RGB, 4 CMYK), and whether it is per channel.
pub(super) fn converter(base: &ColorSpace, channels: usize) -> (Box<dyn Convert>, bool) {
    let process = match channels {
        1 => Process::Gray,
        3 => Process::Rgb,
        _ => Process::Cmyk,
    };
    let target = Target::new(process);
    let converter = Converter::new(base, &target, &Conversion::default()).unwrap();
    let rgb = matches!(base, ColorSpace::DeviceRgb) && process == Process::Rgb;
    (Box::new(converter), rgb)
}

/// The space an image's samples convert from: an Indexed space's base.
pub(super) fn base(space: &ColorSpace) -> &ColorSpace {
    match space {
        ColorSpace::Indexed(indexed) => &indexed.base,
        space => space,
    }
}

/// One image case: samples, interpretation and masking.
#[derive(Clone)]
pub(super) struct Case {
    pub(super) image: DecodedImage,
    pub(super) space: Option<ColorSpace>,
    pub(super) decode: Vec<[f64; 2]>,
    pub(super) key: Option<Vec<[u16; 2]>>,
    pub(super) mask: Option<(DecodedImage, [f64; 2], MaskUse)>,
    pub(super) alpha_is_shape: bool,
    pub(super) stencil_color: Option<Vec<f64>>,
}

#[derive(Clone)]
pub(super) enum MaskUse {
    Stencil,
    Soft(Option<Vec<f64>>),
}

pub(super) fn image(
    (width, height): (u32, u32),
    components: u8,
    bits: u8,
    samples: Samples,
) -> DecodedImage {
    DecodedImage {
        width,
        height,
        bits_per_component: bits,
        components,
        samples,
        kind: ImageKind::Color { color_space: None },
        decode: None,
        mask: MaskKind::None,
        interpolate: false,
        reduction: None,
    }
}

/// Deterministic sample codes of `bits` bits.
fn codes(count: usize, bits: u8, seed: u32) -> Vec<u16> {
    let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(17);
    (0..count)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            u16::try_from((state >> 8) & (u32::from(u16::MAX) >> (16 - bits))).unwrap()
        })
        .collect()
}

fn bytes(count: usize, bits: u8, seed: u32) -> Samples {
    Samples::Bytes(
        codes(count, bits, seed)
            .into_iter()
            .map(|v| u8::try_from(v).unwrap())
            .collect(),
    )
}

fn space(components: usize) -> ColorSpace {
    match components {
        1 => ColorSpace::DeviceGray,
        _ => ColorSpace::DeviceRgb,
    }
}

fn case(image: DecodedImage) -> Case {
    let components = usize::from(image.components);
    Case {
        space: Some(space(components)),
        decode: vec![[0.0, 1.0]; components],
        key: None,
        mask: None,
        alpha_is_shape: false,
        stencil_color: None,
        image,
    }
}

/// The new sampler for `case` under `unit_to_device`, into `target`
/// channels.
fn sampler(case: &Case, unit_to_device: Transform, target: usize) -> ImageSampler {
    let inverse = unit_to_device.inverse().unwrap();
    let window = window(unit_to_device);
    let mask = case.mask.as_ref().map(|(mask, decode, kind)| {
        let description = match kind {
            MaskUse::Stencil => MaskDescription::Stencil,
            MaskUse::Soft(matte) => MaskDescription::Soft {
                decode: *decode,
                matte: matte.as_deref(),
            },
        };
        (Arc::new(Decoded::from(mask.clone())), description)
    });
    let description = Description {
        space: case.space.as_ref(),
        decode: &case.decode,
        key: case.key.as_deref(),
        mask: mask.as_ref().map_or(MaskDescription::None, |(_, d)| *d),
        alpha_is_shape: case.alpha_is_shape,
    };
    let convert = case
        .space
        .as_ref()
        .map(|space| converter(base(space), target));
    // Quarter turns sample the image with rows and columns exchanged, as
    // preparation decodes it.
    let orientation = Orientation::of(unit_to_device, description.mask);
    let image = match orientation {
        Orientation::Rows => case.image.clone(),
        Orientation::Transposed => case
            .image
            .transposed(&pagura_core::Limits::default())
            .unwrap(),
    };
    let (prepared, _) = Prepared::new(
        &description,
        (Arc::new(Decoded::from(image)), mask.map(|(m, _)| m)),
        (inverse, (window.0, window.2), orientation),
        convert,
    )
    .unwrap();
    let sampler = ImageSampler {
        channels: if case.space.is_some() { target } else { 0 },
        prepared: Some(Box::new(prepared)),
    };
    match &case.stencil_color {
        Some(color) => sampler.with_stencil_color(color, Painted(7)),
        None => sampler,
    }
}

/// Placements of a unit square: magnified, minified upright and flipped,
/// quarter turns, rotated and skewed, anisotropic.
fn placements(width: u32, height: u32) -> Vec<Transform> {
    let (w, h) = (f64::from(width), f64::from(height));
    vec![
        Transform::new(3.7 * w, 0.0, 0.0, 3.3 * h, 0.4, 0.3),
        Transform::new(w, 0.0, 0.0, h, 0.0, 0.0),
        Transform::new(w / 2.4, 0.0, 0.0, -h / 3.2, 1.25, 1.0 + h / 3.2),
        Transform::new(-w / 3.0, 0.0, 0.0, h / 2.0, 0.5 + w / 3.0, 0.75),
        Transform::new(0.0, h / 2.5, w / 3.5, 0.0, 0.3, 0.6),
        Transform::new(0.0, -h / 4.0, -w / 2.0, 0.0, 1.5 + w / 2.0, 0.5 + h / 4.0),
        Transform::new(0.0, 2.7 * h, 3.1 * w, 0.0, 0.2, 0.4),
        Transform::new(0.0, -h * 0.9, w * 1.6, 0.0, 0.7, 0.3 + h * 0.9),
        Transform::new(w * 0.31, w * 0.17, -h * 0.13, h * 0.29, 4.5, 0.25),
        Transform::new(w * 1.9, w * 0.4, h * 0.6, -h * 1.7, 0.5, 2.0 * h),
        Transform::new(w / 7.3, 0.0, 0.0, h * 2.1, 0.25, 0.0),
        Transform::new(w * 0.999_999_999_9, 0.0, 0.0, h * 1.000_000_000_2, 0.0, 0.0),
    ]
}

/// The device pixels a placement covers, with a margin.
fn window(unit_to_device: Transform) -> (u32, u32, u32, u32) {
    let corners = [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)]
        .map(|(x, y)| pagura_raster::Point::new(x, y) * unit_to_device);
    let x1 = corners.iter().map(|p| p.x).fold(0.0, f64::max);
    let y1 = corners.iter().map(|p| p.y).fold(0.0, f64::max);
    let clamp = |v: f64| {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "test window"
        )]
        let v = v.ceil().clamp(0.0, 200.0) as u32;
        v
    };
    (0, 0, clamp(x1) + 2, clamp(y1) + 2)
}

/// Whether a sampled value agrees with the oracle's: bit for bit, or, where
/// codes are averaged as integers, within the rounding of a few f64
/// operations.
fn close(actual: f64, expected: f64) -> bool {
    actual.to_bits() == expected.to_bits()
        || (actual - expected).abs() <= 1e-13 * expected.abs().max(1.0)
}

/// Checks that byte spans, where the sampler fills them, are the colours of
/// its f64 spans as `U8` storage rounds them, over every pixel of rows
/// `rows` from column `x0` on, as whole rows and as single pixels. Returns
/// whether it fills byte spans.
pub(super) fn bytes_agree(
    sampler: &ImageSampler,
    (x0, len): (u32, usize),
    rows: std::ops::Range<u32>,
) -> bool {
    if !sampler.has_byte_spans() {
        return false;
    }
    let quantize = pagura_raster::quantize_unit;
    let mut span = SpanColors::default();
    let (mut bytes, mut single) = (vec![[9u8; 4]; len], [[9u8; 4]; 1]);
    for y in rows {
        sampler.span(x0, y, len, &mut span);
        assert_eq!(span.channels, 3);
        assert!(sampler.span_bytes(x0, y, &mut bytes));
        for (i, (x, &actual)) in (x0..).zip(&bytes).enumerate() {
            let alpha = span.alpha.get(i).copied().unwrap_or(1.0);
            assert!(alpha == 0.0 || alpha == 1.0, "opaque byte spans");
            let color = &span.colors[i * 3..i * 3 + 3];
            let expected = if alpha == 0.0 {
                [0; 4]
            } else {
                [
                    quantize(color[0]),
                    quantize(color[1]),
                    quantize(color[2]),
                    255,
                ]
            };
            assert_eq!(actual, expected, "pixel ({x}, {y}) of {color:?}");
            assert!(sampler.span_bytes(x, y, &mut single));
            assert_eq!(single[0], expected, "single pixel ({x}, {y})");
        }
    }
    true
}

/// Compares the new sampler with the oracle on every pixel of the window,
/// as whole rows and as single pixels. Nearest samples and the general
/// paths agree bit for bit; averages of codes within [`close`]. An empty
/// alpha span means every alpha is 1.
fn agree(case: &Case, target: usize) {
    for unit_to_device in placements(case.image.width, case.image.height) {
        let sampler = sampler(case, unit_to_device, target);
        let mut oracle = oracle::Oracle::new(case, unit_to_device, target);
        let (x0, y0, x1, y1) = window(unit_to_device);
        let len = usize::try_from(x1 - x0).unwrap();
        let bytes = bytes_agree(&sampler, (x0, len), y0..y1);
        assert!(!bytes || target == 3, "byte spans fill three channels");
        let (mut row, mut single) = (SpanColors::default(), SpanColors::default());
        let (mut row_shape, mut single_shape) = (Vec::new(), Vec::new());
        for y in y0..y1 {
            sampler.span_with_shape(x0, y, len, &mut row, &mut row_shape);
            let channels = row.channels;
            assert_eq!(row.colors.len(), len * channels);
            assert!(row.alpha.is_empty() || row.alpha.len() == len);
            for (i, x) in (x0..x1).enumerate() {
                let expected = oracle.pixel(x, y);
                let colors = &row.colors[i * channels..(i + 1) * channels];
                let alpha = row.alpha.get(i).copied().unwrap_or(1.0);
                let context = || format!("{unit_to_device:?} pixel ({x}, {y})");
                for (&actual, &expected) in colors.iter().zip(&expected.color[..channels]) {
                    assert!(
                        close(actual, expected),
                        "colour {actual} vs {expected} {}",
                        context()
                    );
                }
                assert!(
                    close(alpha, expected.alpha),
                    "alpha {alpha} vs {} {}",
                    expected.alpha,
                    context()
                );
                assert!(
                    close(row_shape[i], expected.shape),
                    "shape {} vs {} {}",
                    row_shape[i],
                    expected.shape,
                    context()
                );
                if sampler.shape_is_alpha() {
                    assert_eq!(row_shape[i].to_bits(), alpha.to_bits(), "{}", context());
                }
                sampler.span_with_shape(x, y, 1, &mut single, &mut single_shape);
                assert_eq!(single.colors, colors, "partition {}", context());
                assert_eq!(
                    single.alpha.first().copied().unwrap_or(1.0).to_bits(),
                    alpha.to_bits(),
                    "partition {}",
                    context()
                );
            }
        }
    }
}

#[test]
fn gray_images_of_every_precision_and_decode() {
    for bits in [1, 2, 4, 8, 16] {
        for decode in [[0.0, 1.0], [1.0, 0.0], [-0.25, 1.25]] {
            let samples = if bits == 16 {
                Samples::Words(codes(13 * 9, 16, 3))
            } else {
                bytes(13 * 9, bits, 3)
            };
            let mut case = case(image((13, 9), 1, bits, samples));
            case.decode = vec![decode];
            for target in [1, 3] {
                agree(&case, target);
            }
        }
    }
}

#[test]
fn packed_one_bit_samples_sample_as_their_bytes() {
    let width = 11;
    let values = codes(11 * 7, 1, 9);
    let stride = 2;
    let mut packed = vec![0u8; stride * 7];
    for (i, &v) in values.iter().enumerate() {
        let (y, x) = (i / width, i % width);
        packed[y * stride + x / 8] |= u8::try_from(v).unwrap() << (7 - x % 8);
    }
    let packed = pagura_image::deferred::Packed::from_rows(11, 7, packed).unwrap();
    let case_packed = case(image((11, 7), 1, 1, Samples::Packed(packed)));
    agree(&case_packed, 3);
}

#[test]
fn rgb_images_through_channel_tables_and_samples() {
    for bits in [8, 16] {
        let samples = if bits == 16 {
            Samples::Words(codes(9 * 12 * 3, 16, 5))
        } else {
            bytes(9 * 12 * 3, 8, 5)
        };
        let mut case = case(image((9, 12), 3, bits, samples));
        case.decode = vec![[0.0, 1.0], [1.0, 0.0], [0.1, 0.9]];
        agree(&case, 3);
        agree(&case, 1);
    }
}

#[test]
fn indexed_images_look_up_their_base_colours() {
    let lookup: Vec<u8> = (0..=255u8)
        .flat_map(|v| [v, 255 - v, v.wrapping_mul(73)])
        .collect();
    for bits in [2, 8] {
        let mut case = case(image((10, 8), 1, bits, bytes(80, bits, 7)));
        case.space = Some(ColorSpace::Indexed(Arc::new(Indexed {
            base: Arc::new(ColorSpace::DeviceRgb),
            high: 200,
            lookup: lookup.clone().into(),
        })));
        case.decode = vec![[0.0, f64::from((1u16 << bits) - 1)]];
        agree(&case, 3);
    }
}

#[test]
fn colour_keys_mask_matching_samples() {
    let mut rgb = case(image((8, 8), 3, 8, bytes(8 * 8 * 3, 8, 11)));
    rgb.key = Some(vec![[0, 128], [64, 255], [0, 200]]);
    agree(&rgb, 3);
    let mut gray = case(image((8, 8), 1, 4, bytes(64, 4, 11)));
    gray.key = Some(vec![[3, 9]]);
    agree(&gray, 3);
    let mut wide = case(image((6, 5), 3, 16, Samples::Words(codes(90, 16, 2))));
    wide.key = Some(vec![[0, 40_000], [10_000, 65_535], [0, 65_535]]);
    agree(&wide, 3);
}

#[test]
fn stencils_paint_alpha_and_their_colour() {
    for inverted in [false, true] {
        let mut stencil = image((12, 10), 1, 1, bytes(120, 1, 13));
        stencil.kind = ImageKind::Stencil { inverted };
        let mut case = case(stencil);
        case.space = None;
        case.decode = vec![[0.0, 1.0]];
        agree(&case, 3);
        case.stencil_color = Some(vec![0.3, 0.7, 0.123_456_789]);
        agree(&case, 3);
    }
}

#[test]
fn soft_masks_with_and_without_matte_and_stencil_masks() {
    let rgb = image((9, 7), 3, 8, bytes(9 * 7 * 3, 8, 17));
    for (mask_size, bits) in [((9, 7), 8), ((5, 13), 4), ((9, 7), 16)] {
        let count = usize::try_from(mask_size.0 * mask_size.1).unwrap();
        let samples = if bits == 16 {
            Samples::Words(codes(count, 16, 19))
        } else {
            bytes(count, bits, 19)
        };
        let mask = image(mask_size, 1, bits, samples);
        for decode in [[0.0, 1.0], [1.0, 0.0], [0.25, 1.5]] {
            for alpha_is_shape in [false, true] {
                let mut soft = case(rgb.clone());
                soft.alpha_is_shape = alpha_is_shape;
                soft.mask = Some((mask.clone(), decode, MaskUse::Soft(None)));
                agree(&soft, 3);
                if mask_size == (9, 7) {
                    soft.mask = Some((
                        mask.clone(),
                        decode,
                        MaskUse::Soft(Some(vec![1.0, 0.25, 0.0])),
                    ));
                    agree(&soft, 3);
                }
            }
        }
    }
    let mut stencil = image((6, 4), 1, 1, bytes(24, 1, 23));
    stencil.kind = ImageKind::Stencil { inverted: true };
    let mut masked = case(rgb);
    masked.mask = Some((stencil, [1.0, 0.0], MaskUse::Stencil));
    agree(&masked, 3);
}

#[test]
fn embedded_opacity_straight_and_premultiplied() {
    for premultiplied in [false, true] {
        for alpha_is_shape in [false, true] {
            let mut rgb = image((7, 6), 3, 8, bytes(7 * 6 * 3, 8, 29));
            rgb.mask = MaskKind::Embedded(ImageAlpha {
                bits_per_component: 8,
                samples: bytes(42, 8, 31),
                premultiplied,
            });
            let mut case = case(rgb);
            case.alpha_is_shape = alpha_is_shape;
            agree(&case, 3);
        }
    }
}

#[test]
fn interpolated_and_reduced_images() {
    let mut gray = case(image((7, 5), 1, 8, bytes(35, 8, 37)));
    gray.image.interpolate = true;
    agree(&gray, 3);
    let mut cells = case(image((5, 4), 3, 8, bytes(60, 8, 41)));
    cells.image.reduction = Some(pagura_image::Reduction {
        cells: 3,
        width: 13,
        height: 9,
    });
    agree(&cells, 3);
}

#[test]
fn opacity_tables_equal_scalar_opacities() {
    for bits in [1, 2, 4, 8] {
        let mut mask = image((16, 16), 1, bits, bytes(256, bits, 43));
        for decode in [[0.0, 1.0], [1.0, 0.0], [0.3, 0.6]] {
            let opacity = Opacity::new(&mask, decode);
            for index in 0..256 {
                let code = mask.samples.get(index).unwrap();
                let max = f64::from((1u16 << bits) - 1);
                let expected =
                    (decode[0] + f64::from(code) / max * (decode[1] - decode[0])).clamp(0.0, 1.0);
                assert_eq!(opacity.at(&mask, index).to_bits(), expected.to_bits());
            }
        }
        mask.kind = ImageKind::Stencil { inverted: false };
    }
}

#[test]
fn degenerate_placements_paint_nothing_and_overflow_is_refused() {
    let gray = image((4, 4), 1, 8, bytes(16, 8, 47));
    assert!(
        Geometry::new(
            (4, 4),
            (4.0, 4.0),
            false,
            Transform::new(1e300, 0.0, 0.0, 1.0, 0.0, 0.0)
        )
        .is_err()
    );
    let mut empty = SpanColors::default();
    let sampler = ImageSampler {
        prepared: None,
        channels: 3,
    };
    sampler.span(0, 0, 4, &mut empty);
    assert_eq!((empty.colors.len(), empty.alpha), (12, vec![0.0; 4]));
    let _ = gray;
}

#[test]
fn prepared_samplers_are_shared_between_threads() {
    fn shared<T: Send + Sync>() {}
    shared::<ImageSampler>();
}

#[test]
fn device_and_cie_spaces_into_every_process_target() {
    let cmyk = Case {
        space: Some(ColorSpace::DeviceCmyk),
        ..case(image((7, 6), 4, 8, bytes(7 * 6 * 4, 8, 53)))
    };
    let mut cmyk = cmyk;
    cmyk.decode = vec![[0.0, 1.0]; 4];
    let lab = pagura_color::cie::Calibrated::lab(
        [0.9505, 1.0, 1.089],
        [0.0; 3],
        [-100.0, 100.0, -80.0, 90.0],
    )
    .unwrap();
    let mut lab = Case {
        space: Some(ColorSpace::Calibrated(Arc::new(lab))),
        ..case(image((6, 7), 3, 8, bytes(6 * 7 * 3, 8, 59)))
    };
    lab.decode = vec![[0.0, 100.0], [-100.0, 100.0], [-80.0, 90.0]];
    let rgb = case(image((8, 5), 3, 8, bytes(8 * 5 * 3, 8, 61)));
    let gray = case(image((5, 8), 1, 8, bytes(40, 8, 67)));
    for target in [1, 3, 4] {
        for case in [&cmyk, &lab, &rgb, &gray] {
            agree(case, target);
        }
    }
}

/// The bits of `values`.
fn bits(values: &[f64]) -> Vec<u64> {
    values.iter().map(|v| v.to_bits()).collect()
}

/// Spans of `len` pixels from `x0`: single pixels from the right or, on
/// odd rows, from the left, then halves and the whole row, so that kept
/// rows extend both ways and serve other ranges.
fn partitions(x0: u32, len: usize, y: u32) -> Vec<(u32, usize)> {
    let at = |i: usize| x0 + u32::try_from(i).unwrap();
    let mut spans: Vec<_> = (0..len).map(|i| (at(i), 1)).collect();
    if y.is_multiple_of(2) {
        spans.reverse();
    }
    let half = len / 2;
    spans.extend([(at(half), len - half), (x0, half), (x0, len)]);
    spans
}

/// Source rows summed row by row, with texels from channel and Matte
/// tables, give every pixel bit for bit what summing each pixel's
/// per-sample texels gives, the image's and the mask's, for every partition
/// of the device row.
#[test]
fn rows_and_tables_sum_as_samples_do() {
    let rgb = image((23, 17), 3, 8, bytes(23 * 17 * 3, 8, 71));
    let gray = image((23, 17), 1, 8, bytes(23 * 17, 8, 73));
    // Mostly opaque, with transparent and partly covered samples.
    let mask = image(
        (23, 17),
        1,
        8,
        Samples::Bytes(
            codes(23 * 17, 8, 79)
                .into_iter()
                .map(|v| match v % 4 {
                    0 => 0,
                    1 => u8::try_from(v >> 8).unwrap(),
                    _ => 255,
                })
                .collect(),
        ),
    );
    let mut cases = Vec::new();
    for source in [&rgb, &gray] {
        for matte in [None, Some(vec![0.0; 3]), Some(vec![1.0, 0.25, 0.0])] {
            for decode in [[0.0, 1.0], [0.25, 0.75]] {
                let mut soft = case(source.clone());
                let matte = matte
                    .clone()
                    .map(|m| m[..usize::from(source.components)].to_vec());
                soft.mask = Some((mask.clone(), decode, MaskUse::Soft(matte)));
                cases.push(soft);
            }
        }
    }
    let flipped =
        |w: f64, h: f64| Transform::new(w / 2.78, 0.0, 0.0, -h / 2.78, 0.5, 1.0 + h / 2.78);
    for case in &cases {
        let (w, h) = (f64::from(case.image.width), f64::from(case.image.height));
        let mut placements = placements(case.image.width, case.image.height);
        placements.push(flipped(w, h));
        placements.push(Transform::new(w / 1.34, 0.0, 0.0, h / 1.34, 0.3, 0.2));
        for target in [1, 3, 4] {
            for &unit_to_device in &placements {
                let sampler = sampler(case, unit_to_device, target);
                let prepared = sampler.prepared.as_deref().unwrap();
                let Paint::Colors(paint) = &prepared.paint else {
                    panic!("colour image");
                };
                let (colors, _) = paint.as_ref();
                let mask = prepared.mask.as_ref().unwrap();
                let lanes = colors.lanes();
                let (x0, y0, x1, y1) = window(unit_to_device);
                let len = usize::try_from(x1 - x0).unwrap();
                for y in y0..y1 {
                    for (x, len) in partitions(x0, len, y) {
                        let span = (x, y, len);
                        let mut rows = vec![0.0; len * lanes];
                        prepared.texels(span, lanes, &mut rows);
                        let matte = colors
                            .matte
                            .as_ref()
                            .map(|_| (&mask.image.image, &mask.opacity));
                        let texels = SampleTexels::new(&prepared.image.image, colors, matte, None);
                        let mut samples = vec![0.0; len * lanes];
                        prepared.geometry.fill(span, &texels, &mut samples);
                        let context = format!("{unit_to_device:?} target {target} span {span:?}");
                        assert_eq!(bits(&rows), bits(&samples), "{context}");
                        let opacity = OpacityTexels {
                            mask: &mask.image.image,
                            opacity: &mask.opacity,
                        };
                        let (mut rows, mut samples) = (vec![0.0; len], vec![0.0; len]);
                        mask.lines.fill(&mask.geometry, span, &opacity, &mut rows);
                        mask.geometry.fill(span, &opacity, &mut samples);
                        assert_eq!(bits(&rows), bits(&samples), "mask {context}");
                    }
                }
            }
        }
    }
}

/// Opaque gray and RGB images of every precision fill byte spans into an
/// RGB space, at every placement the codes sample; masked and translucent
/// images do not.
#[test]
fn byte_spans_cover_opaque_images_sampled_by_codes() {
    let rgb = case(image((9, 7), 3, 8, bytes(9 * 7 * 3, 8, 83)));
    let mut filled = 0;
    for bits in [1, 2, 4, 8] {
        let gray = case(image((13, 9), 1, bits, bytes(13 * 9, bits, 89)));
        for interpolate in [false, true] {
            for case in [&gray, &rgb] {
                let mut case = case.clone();
                case.image.interpolate = interpolate;
                for unit_to_device in placements(case.image.width, case.image.height) {
                    let sampler = sampler(&case, unit_to_device, 3);
                    let (x0, y0, x1, y1) = window(unit_to_device);
                    let len = usize::try_from(x1 - x0).unwrap();
                    filled += usize::from(bytes_agree(&sampler, (x0, len), y0..y1));
                }
            }
        }
    }
    // Every upright, flipped and quarter-turned placement; not the rotated
    // and skewed ones.
    assert_eq!(filled, 4 * 2 * 2 * 10);
    let mut masked = rgb.clone();
    masked.mask = Some((
        image((9, 7), 1, 8, bytes(63, 8, 97)),
        [0.0, 1.0],
        MaskUse::Soft(None),
    ));
    let unit_to_device = Transform::new(4.5, 0.0, 0.0, 3.5, 0.0, 0.0);
    assert!(!sampler(&masked, unit_to_device, 3).has_byte_spans());
    assert!(!sampler(&rgb, unit_to_device, 1).has_byte_spans());
}

/// A packed one-bit image of `width` × `height` samples, mostly ones when
/// `white`.
fn packed_image((width, height): (u32, u32), white: bool, seed: u32) -> DecodedImage {
    let (w, h) = (
        usize::try_from(width).unwrap(),
        usize::try_from(height).unwrap(),
    );
    let stride = w.div_ceil(8);
    let values = codes(w * h, 8, seed);
    let mut rows = vec![0u8; stride * h];
    for (i, &v) in values.iter().enumerate() {
        let (y, x) = (i / w, i % w);
        // Mostly white: blocks of white between blocks of noise.
        let one = if white {
            (x / 9 + y / 7) % 3 != 0 || v % 2 == 1
        } else {
            v % 2 == 1
        };
        rows[y * stride + x / 8] |= u8::from(one) << (7 - x % 8);
    }
    let packed = pagura_image::deferred::Packed::from_rows(w, h, rows).unwrap();
    image((width, height), 1, 1, Samples::Packed(packed))
}

/// Packed one-bit images fill byte spans from counts of their bits: whole
/// two-by-two means, and footprints of every size and phase, at both
/// decodes, each byte the rounded f64 colour.
#[test]
fn packed_bits_fill_bytes_as_their_colours_round() {
    let (w, h) = (70.0, 53.0);
    let placements = [
        Transform::new(w / 2.0, 0.0, 0.0, h / 2.0, 0.0, 0.0),
        Transform::new(w / 2.0, 0.0, 0.0, -h / 2.0, 3.0, 1.0 + h / 2.0),
        Transform::new(w / 4.17, 0.0, 0.0, -h / 4.17, 0.3, 0.7 + h / 4.17),
        Transform::new(w / 1.5, 0.0, 0.0, h / 1.7, 0.25, 0.4),
        Transform::new(w / 7.3, 0.0, 0.0, h / 9.1, 0.6, 0.1),
        Transform::new(w / 2.0, 0.0, 0.0, h / 2.0, 0.5, 0.0),
        Transform::new(0.0, -h / 2.0, -w / 2.0, 0.0, w / 2.0, h / 2.0),
        Transform::new(0.0, h / 4.3, w / 3.9, 0.0, 0.2, 0.3),
    ];
    for white in [false, true] {
        for decode in [[0.0, 1.0], [1.0, 0.0]] {
            let mut bits = case(packed_image((70, 53), white, 101));
            bits.decode = vec![decode];
            for &unit_to_device in &placements {
                let sampler = sampler(&bits, unit_to_device, 3);
                let (x0, y0, x1, y1) = window(unit_to_device);
                let len = usize::try_from(x1 - x0).unwrap();
                assert!(bytes_agree(&sampler, (x0, len), y0..y1));
                // The columns inside the image, whose footprints are all
                // whole where the scale is two.
                let corners = [(0.0, 0.0), (1.0, 1.0)]
                    .map(|(x, y)| pagura_raster::Point::new(x, y) * unit_to_device);
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "test columns"
                )]
                let (left, right) = (
                    corners[0].x.min(corners[1].x).ceil() as u32,
                    corners[0].x.max(corners[1].x).floor() as u32,
                );
                let inside = usize::try_from(right - left).unwrap();
                assert!(bytes_agree(&sampler, (left, inside), y0..y1));
                // Spans of a few pixels from every start.
                for start in x0..x1 {
                    let len = usize::try_from((x1 - start).min(5)).unwrap();
                    bytes_agree(&sampler, (start, len), y0..y0 + 3);
                }
            }
        }
    }
}
