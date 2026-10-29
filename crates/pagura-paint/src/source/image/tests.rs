//! The sampler family against the exact general route it replaces.
//!
//! `oracle` is a port of the old exact route's formulas
//! (`pagura-render`'s `compositor/sampled.rs`, `compositor/reduced.rs` and
//! the texel code of `compositor/images.rs`), kept in their original
//! structure: per-pixel sampling with the streamed column caches, `[f64; 4]`
//! texels and the separate mask sampler. Both sides convert colours with the
//! same device-space double, so every colour, alpha and shape must agree
//! bit for bit.

use super::geometry::{BilinearRows, Geometry, Texels};
use super::texels::{Opacity, OpacityTexels, SampleTexels, TexelMode};
use super::{Description, ImageSampler, MaskDescription, Orientation, Paint, Prepared};
use crate::source::{PaintScratch, Pixels, ScratchLayout, SpanColors};
use pagura_color::{ColorSpace, Conversion, Converter, Indexed, Painted, Process, Target};
use pagura_image::{Decoded, DecodedImage, ImageAlpha, ImageKind, MaskKind, Samples};
use pagura_raster::Transform;
use std::sync::Arc;

mod accuracy;
mod documents;
mod oracle;
mod ownership;
mod timing;

pub(super) fn meter() -> pagura_core::Meter {
    pagura_core::Meter::new(usize::MAX, "test")
}

/// A predecoded gray image or stencil, sharing its input as a Store hit does.
fn prepared_with_budget(
    stencil: bool,
    color: Option<&[f64]>,
    bytes: &mut pagura_core::Meter,
) -> pagura_core::Result<ImageSampler> {
    let mut decoded = image((5, 3), 1, 8, Samples::Bytes(vec![0; 15]));
    if stencil {
        decoded.kind = ImageKind::Stencil { inverted: false };
        decoded.bits_per_component = 1;
    }
    let decoded = Arc::new(Decoded::from(decoded));
    bytes.charge(super::decoded_bytes(&decoded.image))?;
    let space = ColorSpace::DeviceGray;
    let description = Description {
        space: (!stencil).then_some(&space),
        decode: &[[0.0, 1.0]],
        key: None,
        mask: MaskDescription::None,
        alpha_is_shape: false,
    };
    let convert = (!stencil).then(|| converter(&space, 3));
    let prepared = Prepared::new(
        &description,
        (decoded, None),
        (Transform::scale(0.05, 0.1), (0, 20), Orientation::Rows),
        convert,
        color.map(|color| (color, Painted(7))),
        1,
        super::ImageBudgets {
            bytes,
            work: &mut meter(),
            functions: &mut meter(),
        },
    )?;
    Ok(ImageSampler::new(
        Some(Box::new(prepared)),
        if stencil {
            color.map_or(0, <[f64]>::len)
        } else {
            3
        },
    ))
}

#[test]
fn immutable_code_plans_release_construction_storage() {
    let mut bytes = meter();
    let sampler = prepared_with_budget(false, None, &mut bytes).unwrap();
    let retained = sampler.bytes();
    assert!(matches!(
        sampler.prepared.as_ref().unwrap().plan,
        super::Plan::Codes(_)
    ));
    assert_eq!(bytes.used(), retained);
    // The copied code lanes and original conversion tables overlap during
    // construction, even though the returned sampler keeps only its lanes.
    let mut exact = pagura_core::Meter::new(retained, "test");
    assert!(prepared_with_budget(false, None, &mut exact).is_err());
}

#[test]
fn masks_own_a_plan_only_for_independent_sampling() {
    for ((width, height), interpolate) in [((5, 3), false), ((7, 4), false), ((5, 3), true)] {
        let independent = (width, height) != (5, 3) || interpolate;
        let make = |budget: &mut pagura_core::Meter, work: &mut pagura_core::Meter| {
            let decoded = Arc::new(Decoded::from(image((5, 3), 1, 8, bytes(15, 8, 1))));
            let mut mask = image(
                (width, height),
                1,
                8,
                bytes(usize::try_from(width * height).unwrap(), 8, 2),
            );
            mask.interpolate = interpolate;
            let mask = Arc::new(Decoded::from(mask));
            budget.charge(super::decoded_bytes(&decoded.image))?;
            budget.charge(super::decoded_bytes(&mask.image))?;
            Prepared::new(
                &Description {
                    space: Some(&ColorSpace::DeviceGray),
                    decode: &[[0.0, 1.0]],
                    key: None,
                    mask: MaskDescription::Soft {
                        decode: [0.0, 1.0],
                        matte: None,
                    },
                    alpha_is_shape: false,
                },
                (decoded, Some(mask)),
                (Transform::scale(0.05, 0.1), (0, 20), Orientation::Rows),
                Some(converter(&ColorSpace::DeviceGray, 3)),
                None,
                1,
                super::ImageBudgets {
                    bytes: budget,
                    work,
                    functions: &mut meter(),
                },
            )
        };
        let mut bytes = meter();
        let mut work = meter();
        let prepared = make(&mut bytes, &mut work).unwrap();
        let mask = prepared.mask.as_ref().unwrap();
        assert_eq!(mask.independent.is_some(), independent);
        assert_eq!(bytes.used(), prepared.bytes());
        let mut exact = pagura_core::Meter::new(bytes.used(), "test");
        assert_eq!(
            make(&mut exact, &mut meter()).unwrap().bytes(),
            bytes.used()
        );
        let mask_cost = mask
            .independent
            .as_ref()
            .map_or(prepared.geometry.cost(), |(g, _)| g.cost());
        assert_eq!(work.used(), prepared.geometry.cost() + mask_cost);
    }
}

#[test]
fn stencil_recolouring_charges_overlap_and_releases_replaced_tables() {
    for color in [None, Some([0.25, 0.5, 0.75].as_slice())] {
        let mut bytes = meter();
        let sampler = prepared_with_budget(true, color, &mut bytes).unwrap();
        assert_eq!(bytes.used(), sampler.bytes());
        let mut exact = pagura_core::Meter::new(sampler.bytes(), "test");
        assert!(prepared_with_budget(true, color, &mut exact).is_err());
    }
}

/// Allocates exactly the layout that preparation reserved.
pub(super) fn scratch(sampler: &ImageSampler) -> super::ImageScratch {
    let layout = sampler.scratch_layout();
    let scratch = layout.make().unwrap();
    assert_eq!(scratch.bytes(), layout.bytes());
    scratch
}

/// A prepared conversion of `base` into a composite target of `channels`
/// process channels (1 Gray, 3 RGB, 4 CMYK), and whether it is per channel.
pub(super) fn converter(base: &ColorSpace, channels: usize) -> (Converter, bool) {
    let process = match channels {
        1 => Process::Gray,
        3 => Process::Rgb,
        _ => Process::Cmyk,
    };
    let target = Target::new(process);
    let converter = Converter::new(base, &target, &Conversion::default()).unwrap();
    let rgb = matches!(base, ColorSpace::DeviceRgb) && process == Process::Rgb;
    (converter, rgb)
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
    let mut sampler = prepared_sampler(case, unit_to_device, target);
    // A page with room to spare keeps the optional curve values.
    sampler.admit_curves(&mut meter());
    sampler
}

/// [`sampler`] before the optional curve values are admitted.
fn prepared_sampler(case: &Case, unit_to_device: Transform, target: usize) -> ImageSampler {
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
        // A quarter turn samples the mask with rows and columns exchanged
        // too.
        let mask = match Orientation::of(unit_to_device) {
            Orientation::Rows => mask.clone(),
            Orientation::Transposed => mask.transposed(&pagura_core::Limits::default()).unwrap(),
        };
        (Arc::new(Decoded::from(mask)), description)
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
    let orientation = Orientation::of(unit_to_device);
    let image = match orientation {
        Orientation::Rows => case.image.clone(),
        Orientation::Transposed => case
            .image
            .transposed(&pagura_core::Limits::default())
            .unwrap(),
    };
    let prepared = Prepared::new(
        &description,
        (Arc::new(Decoded::from(image)), mask.map(|(m, _)| m)),
        (inverse, (window.0, window.2), orientation),
        convert,
        case.stencil_color
            .as_deref()
            .map(|color| (color, Painted(7))),
        usize::try_from((window.2 - window.0) * (window.3 - window.1)).unwrap(),
        super::ImageBudgets {
            bytes: &mut meter(),
            work: &mut meter(),
            functions: &mut meter(),
        },
    )
    .unwrap();
    ImageSampler::new(
        Some(Box::new(prepared)),
        if case.space.is_some() {
            target
        } else {
            case.stencil_color.as_ref().map_or(0, Vec::len)
        },
    )
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

/// The part of device pixel (`x`, `y`) inside the unit square placed by
/// `unit_to_device`: the coverage its values composite with.
fn coverage(unit_to_device: Transform, (x, y): (u32, u32)) -> f64 {
    let inverse = unit_to_device.inverse().unwrap();
    let (x, y) = (f64::from(x), f64::from(y));
    let mut polygon: Vec<pagura_raster::Point> =
        [(x, y), (x + 1.0, y), (x + 1.0, y + 1.0), (x, y + 1.0)]
            .iter()
            .map(|&(x, y)| pagura_raster::Point::new(x, y) * inverse)
            .collect();
    for (vertical, bound, sign) in [
        (true, 0.0, 1.0),
        (true, 1.0, -1.0),
        (false, 0.0, 1.0),
        (false, 1.0, -1.0),
    ] {
        let side = |p: pagura_raster::Point| sign * (if vertical { p.x } else { p.y } - bound);
        let mut next = Vec::new();
        for (i, &a) in polygon.iter().enumerate() {
            let b = polygon[(i + 1) % polygon.len()];
            if side(a) >= 0.0 {
                next.push(a);
            }
            if (side(a) >= 0.0) != (side(b) >= 0.0) {
                let t = side(a) / (side(a) - side(b));
                next.push(pagura_raster::Point::new(
                    a.x + t * (b.x - a.x),
                    a.y + t * (b.y - a.y),
                ));
            }
        }
        polygon = next;
        if polygon.is_empty() {
            return 0.0;
        }
    }
    let twice: f64 = (0..polygon.len())
        .map(|i| {
            let (a, b) = (polygon[i], polygon[(i + 1) % polygon.len()]);
            a.x * b.y - a.y * b.x
        })
        .sum();
    (twice.abs() * 0.5 * unit_to_device.determinant().abs()).min(1.0)
}

/// Compares the new sampler with the oracle on every pixel of the window
/// with a coverage byte, as whole rows and as single pixels, within
/// [`close`]: colours premultiplied and times the pixel's coverage, as
/// they composite, since a contour average of the image's sliver of a
/// pixel carries its rounding divided by the sliver's area. An empty alpha
/// span means every alpha is 1.
fn agree(case: &Case, target: usize) {
    for unit_to_device in placements(case.image.width, case.image.height) {
        let sampler = sampler(case, unit_to_device, target);
        let mut storage = scratch(&sampler);
        let reserved = storage.bytes();
        let capacities = |storage: &super::ImageScratch| {
            [
                storage.plan.bytes(),
                storage
                    .mask
                    .as_ref()
                    .map_or(0, super::sampling::Scratch::bytes),
                storage.memo.as_ref().map_or(0, |memo| memo.bytes()),
                storage.summed.capacity() * 8,
                storage.fused.capacity() * 8,
            ]
        };
        let before = capacities(&storage);
        let mut oracle = oracle::Oracle::new(case, unit_to_device, target);
        let (x0, y0, x1, y1) = window(unit_to_device);
        let len = usize::try_from(x1 - x0).unwrap();
        let (mut row, mut single) = (SpanColors::default(), SpanColors::default());
        let (mut row_shape, mut single_shape) = (Vec::new(), Vec::new());
        for y in y0..y1 {
            sampler.span_with_shape(
                Pixels::new(x0, y, len),
                &mut storage,
                &mut row,
                &mut row_shape,
            );
            assert_eq!(
                storage.bytes(),
                reserved,
                "sampling keeps its reserved capacities: {unit_to_device:?} target {target}, {:?}; before {before:?}, after {:?}",
                storage.layout(),
                capacities(&storage)
            );
            let channels = row.channels;
            assert_eq!(row.colors.len(), len * channels);
            assert!(row.alpha.is_empty() || row.alpha.len() == len);
            for (i, x) in (x0..x1).enumerate() {
                let weight = coverage(unit_to_device, (x, y));
                if weight * 255.0 < 0.5 {
                    continue;
                }
                let expected = oracle.pixel(x, y);
                let colors = &row.colors[i * channels..(i + 1) * channels];
                let alpha = row.alpha.get(i).copied().unwrap_or(1.0);
                let context = || format!("{unit_to_device:?} pixel ({x}, {y})");
                for (&actual, &color) in colors.iter().zip(&expected.color[..channels]) {
                    assert!(
                        close(actual * alpha * weight, color * expected.alpha * weight),
                        "colour {actual} vs {color} {}",
                        context()
                    );
                }
                assert!(
                    close(alpha * weight, expected.alpha * weight),
                    "alpha {alpha} vs {} {}",
                    expected.alpha,
                    context()
                );
                assert!(
                    close(row_shape[i] * weight, expected.shape * weight),
                    "shape {} vs {} {}",
                    row_shape[i],
                    expected.shape,
                    context()
                );
                if sampler.shape_is_alpha() {
                    assert_eq!(row_shape[i].to_bits(), alpha.to_bits(), "{}", context());
                }
                sampler.span_with_shape(
                    Pixels::new(x, y, 1),
                    &mut storage,
                    &mut single,
                    &mut single_shape,
                );
                assert_eq!(
                    storage.bytes(),
                    reserved,
                    "partitioned sampling keeps its capacities"
                );
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

/// Rows of a packed one-bit mask read, bit for bit, the opacities of its
/// samples: spans from inside a byte, at both edges, past the last row and
/// past the row's end.
#[test]
fn packed_mask_rows_read_the_opacity_of_their_samples() {
    let (width, height) = (13_usize, 6_usize);
    let stride = width.div_ceil(8);
    let mut rows = vec![0u8; stride * height];
    for (i, code) in codes(width * height, 1, 23).into_iter().enumerate() {
        let (y, x) = (i / width, i % width);
        rows[y * stride + x / 8] |= u8::try_from(code).unwrap() << (7 - x % 8);
    }
    let packed = pagura_image::deferred::Packed::from_rows(width, height, rows).unwrap();
    let mask = image((13, 6), 1, 1, Samples::Packed(packed));
    for decode in [[0.0, 1.0], [1.0, 0.0]] {
        let opacity = Opacity::new(&mask, decode, &mut meter()).unwrap();
        let texels = OpacityTexels {
            mask: &mask,
            opacity: &opacity,
        };
        for (row, first, end) in [(0, 0, 13), (2, 3, 11), (5, 7, 13), (6, 0, 13), (1, 9, 15)] {
            let mut values = vec![0.0; usize::try_from(end - first).unwrap()];
            texels.row((row, 13), (first, end), &mut values);
            for (value, x) in values.iter().zip(first..end) {
                let index = usize::try_from(row * 13 + x).unwrap();
                let expected = opacity.at(&mask, index);
                assert_eq!(value.to_bits(), expected.to_bits(), "row {row}, sample {x}");
            }
        }
    }
}

#[test]
fn packed_stencils_sample_as_their_bytes_bit_for_bit() {
    // Rows of three bytes: a head inside a byte, whole bytes and a tail.
    let (width, height) = (21_usize, 9_usize);
    let values = codes(width * height, 1, 31);
    let stride = width.div_ceil(8);
    let mut packed = vec![0u8; stride * height];
    for (i, &v) in values.iter().enumerate() {
        let (y, x) = (i / width, i % width);
        packed[y * stride + x / 8] |= u8::try_from(v).unwrap() << (7 - x % 8);
    }
    let packed = pagura_image::deferred::Packed::from_rows(21, 9, packed).unwrap();
    let bytes = values.iter().map(|&v| u8::try_from(v).unwrap()).collect();
    let stencil = |samples, inverted, color: &Option<Vec<f64>>| {
        let mut stencil = image((21, 9), 1, 1, samples);
        stencil.kind = ImageKind::Stencil { inverted };
        let mut case = case(stencil);
        case.space = None;
        case.decode = vec![[0.0, 1.0]];
        case.stencil_color.clone_from(color);
        case
    };
    for inverted in [false, true] {
        for color in [None, Some(vec![0.3, 0.7, 0.123_456_789])] {
            let packed = stencil(Samples::Packed(packed.clone()), inverted, &color);
            let bytes = stencil(Samples::Bytes(Vec::clone(&bytes)), inverted, &color);
            for unit_to_device in placements(21, 9) {
                let (a, b) = (
                    sampler(&packed, unit_to_device, 3),
                    sampler(&bytes, unit_to_device, 3),
                );
                let (mut scratch_a, mut scratch_b) = (scratch(&a), scratch(&b));
                let (mut span_a, mut span_b) = (SpanColors::default(), SpanColors::default());
                let (x0, y0, x1, y1) = window(unit_to_device);
                let len = usize::try_from(x1 - x0).unwrap();
                for y in y0..y1 {
                    a.span(Pixels::new(x0, y, len), &mut scratch_a, &mut span_a);
                    b.span(Pixels::new(x0, y, len), &mut scratch_b, &mut span_b);
                    let context = format!("{unit_to_device:?} row {y}");
                    assert_eq!(bits(&span_a.colors), bits(&span_b.colors), "{context}");
                    assert_eq!(bits(&span_a.alpha), bits(&span_b.alpha), "{context}");
                }
            }
        }
    }
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

/// Whether every pixel of `sampler`'s window, placed by `unit_to_device`,
/// spans as its uniform colour, opaque, when it has one; returns that it
/// has one.
fn spans_are_uniform(sampler: &ImageSampler, unit_to_device: Transform) -> bool {
    let Some((color, painted)) = sampler.constant(&mut meter(), &mut meter()).unwrap() else {
        return false;
    };
    let (x0, y0, x1, y1) = window(unit_to_device);
    let len = usize::try_from(x1 - x0).unwrap();
    let mut span = SpanColors::default();
    let mut storage = scratch(sampler);
    for y in y0..y1 {
        sampler.span(Pixels::new(x0, y, len), &mut storage, &mut span);
        assert_eq!(span.painted, painted);
        assert!(span.alpha.iter().all(|&alpha| alpha == 1.0));
        for pixel in span.colors.chunks_exact(span.channels) {
            assert_eq!(bits(pixel), bits(&color), "row {y}");
        }
    }
    true
}

#[test]
fn one_texel_images_sampled_nearest_are_uniform() {
    let rgb = case(image((1, 1), 3, 8, Samples::Bytes(vec![40, 200, 77])));
    let placements = placements(1, 1);
    let uniform = placements
        .iter()
        .filter(|&&t| spans_are_uniform(&sampler(&rgb, t, 3), t))
        .count();
    assert!((1..placements.len()).contains(&uniform));
    let magnified = placements[0];
    let mut repaired = sampler(&rgb, magnified, 3);
    Arc::get_mut(&mut repaired.prepared.as_mut().unwrap().image)
        .unwrap()
        .diagnostics
        .push(pagura_core::Diagnostic::information(
            "image.observation",
            "a retained decode observation",
        ));
    assert!(
        repaired
            .constant(&mut meter(), &mut meter())
            .unwrap()
            .is_none()
    );
    assert_eq!(repaired.repairs().len(), 1);
    for inverted in [false, true] {
        let mut stencil = image((1, 1), 1, 1, Samples::Bytes(vec![0]));
        stencil.kind = ImageKind::Stencil { inverted };
        let mut case = case(stencil);
        case.space = None;
        case.stencil_color = Some(vec![0.3, 0.7, 0.123_456_789]);
        // The texel paints unless the mask is inverted.
        let sampler = sampler(&case, magnified, 3);
        assert_eq!(spans_are_uniform(&sampler, magnified), !inverted);
    }
    // Interpolated texels are weighted, and wider images vary.
    let mut interpolated = rgb.clone();
    interpolated.image.interpolate = true;
    assert!(
        sampler(&interpolated, magnified, 3)
            .constant(&mut meter(), &mut meter())
            .unwrap()
            .is_none()
    );
    let wide = case(image(
        (2, 1),
        3,
        8,
        Samples::Bytes(vec![40, 200, 77, 40, 200, 77]),
    ));
    assert!(
        sampler(&wide, magnified, 3)
            .constant(&mut meter(), &mut meter())
            .unwrap()
            .is_none()
    );
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
    let mut masked = case(rgb.clone());
    masked.mask = Some((stencil, [1.0, 0.0], MaskUse::Stencil));
    agree(&masked, 3);
    // A mask on the image's grid with its own `Interpolate`, and Matte.
    for interpolate in [false, true] {
        let mut image = rgb.clone();
        image.interpolate = interpolate;
        let mut mask = image_mask((9, 7));
        mask.interpolate = !interpolate;
        let mut soft = case(image);
        soft.mask = Some((mask, [0.0, 1.0], MaskUse::Soft(Some(vec![0.0; 3]))));
        agree(&soft, 3);
    }
}

/// A mask on the image's grid with its own `Interpolate` averages with the
/// image's footprints where the placement is minified, since an area
/// average does not read it: every colour, alpha and shape has the bits of
/// sampling the mask apart.
#[test]
fn minified_masks_with_their_own_interpolate_average_with_the_image() {
    let rgb = image((9, 7), 3, 8, bytes(9 * 7 * 3, 8, 17));
    let mut mask = image_mask((9, 7));
    mask.interpolate = true;
    for matte in [None, Some(vec![0.0; 3])] {
        let mut soft = case(rgb.clone());
        soft.mask = Some((mask.clone(), [0.0, 1.0], MaskUse::Soft(matte)));
        for unit_to_device in placements(9, 7) {
            let fused = sampler(&soft, unit_to_device, 3);
            let mut apart = sampler(&soft, unit_to_device, 3);
            let prepared = apart.prepared.as_deref_mut().unwrap();
            if prepared.geometry.mode() != super::Mode::Area {
                continue;
            }
            let masked = prepared.mask.as_mut().unwrap();
            assert!(masked.independent.is_none(), "{unit_to_device:?} fuses");
            let (x0, y0, x1, y1) = window(unit_to_device);
            let orientation = Orientation::of(unit_to_device);
            let inverse = unit_to_device.inverse().unwrap();
            let geometry = super::placement(&masked.image.image, inverse, orientation).unwrap();
            let plan = super::Sampling::new(&geometry, (x0, x1), &mut meter()).unwrap();
            masked.independent = Some((geometry, plan));
            let len = usize::try_from(x1 - x0).unwrap();
            let (mut a, mut b) = (scratch(&fused), scratch(&apart));
            for y in y0..y1 {
                let row = |sampler: &ImageSampler, storage: &mut super::ImageScratch| {
                    let (mut colors, mut shape) = (SpanColors::default(), Vec::new());
                    let pixels = Pixels::new(x0, y, len);
                    sampler.span_with_shape(pixels, storage, &mut colors, &mut shape);
                    (bits(&colors.colors), bits(&colors.alpha), bits(&shape))
                };
                assert_eq!(
                    row(&fused, &mut a),
                    row(&apart, &mut b),
                    "{unit_to_device:?} row {y}"
                );
            }
        }
    }
}

/// An eight-bit soft mask of `size`.
fn image_mask(size: (u32, u32)) -> DecodedImage {
    let count = usize::try_from(size.0 * size.1).unwrap();
    image(size, 1, 8, bytes(count, 8, 37))
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
fn interpolated_images() {
    let mut gray = case(image((7, 5), 1, 8, bytes(35, 8, 37)));
    gray.image.interpolate = true;
    agree(&gray, 3);
}

/// Upright bilinear spans read their two source rows once, and every
/// pixel keeps the bits of adding its four texels one at a time: magnified,
/// mirrored and nearly unit placements, taps clamped at the image's edges,
/// texels with and without their opaque alpha and shape lanes.
#[test]
fn upright_bilinear_rows_add_as_single_texels_do() {
    let mut gray = case(image((7, 5), 1, 8, bytes(35, 8, 41)));
    gray.image.interpolate = true;
    // A soft mask on the image's grid keeps converted texels.
    let mask = image((7, 5), 1, 8, bytes(35, 8, 43));
    gray.mask = Some((mask, [0.0, 1.0], MaskUse::Soft(None)));
    let Samples::Bytes(codes) = &gray.image.samples else {
        panic!("byte samples");
    };
    let table: Vec<f64> = (0..256u16)
        .flat_map(|c| {
            let v = f64::from(c) / 255.0;
            [v, v / 3.0, 1.0 - v, 1.0, 1.0]
        })
        .collect();
    let opaque = super::texels::TableTexels {
        codes: super::texels::ByteCodes(codes),
        table: &table,
        lanes: 5,
        opaque: true,
    };
    let (w, h) = (7.0, 5.0);
    for unit_to_device in [
        Transform::new(3.7 * w, 0.0, 0.0, 3.3 * h, 0.4, 0.3),
        Transform::new(-2.9 * w, 0.0, 0.0, -2.1 * h, 0.5 + 2.9 * w, 0.25 + 2.1 * h),
        Transform::new(w * 1.000_000_000_2, 0.0, 0.0, h * 1.5, 0.0, 0.0),
    ] {
        let sampler = sampler(&gray, unit_to_device, 3);
        let prepared = sampler.prepared.as_deref().unwrap();
        let geometry = &prepared.geometry;
        let super::Plan::Texels {
            paint: Paint::Colors(paint),
            ..
        } = &prepared.plan
        else {
            panic!("colour texels");
        };
        let general = SampleTexels::new(&prepared.image.image, &paint.0, None);
        let mask = prepared.mask.as_ref().unwrap();
        let mut values = Vec::new();
        let fused = super::texels::Fused {
            texels: &general,
            mask: OpacityTexels {
                mask: &mask.image.image,
                opacity: &mask.opacity,
            },
            values: std::cell::RefCell::new(&mut values),
        };
        let (x0, y0, x1, y1) = window(unit_to_device);
        let cells = geometry.row_cells((x0, x1));
        assert!(cells > 0, "{unit_to_device:?} reads rows");
        // Each source keeps its rows from span to span and row to row.
        let kept = |lanes: usize| BilinearRows::new(Vec::with_capacity(2 * cells * lanes));
        let mut rows = [kept(general.lanes()), kept(fused.lanes()), kept(5)];
        for y in y0..y1 {
            for (x, len) in partitions(x0, usize::try_from(x1 - x0).unwrap(), y) {
                let pixels = Pixels::new(x, y, len);
                let check = |texels: &dyn Fn(&mut [f64], &mut BilinearRows),
                             lanes,
                             rows: &mut _| {
                    let (mut read, mut single) = (vec![0.0; len * lanes], vec![0.0; len * lanes]);
                    texels(&mut read, rows);
                    texels(&mut single, &mut BilinearRows::default());
                    assert!(rows.read_any(), "the span reads its rows");
                    assert_eq!(
                        bits(&read),
                        bits(&single),
                        "{unit_to_device:?} ({x}, {y}) {len}"
                    );
                };
                let [general_rows, fused_rows, opaque_rows] = &mut rows;
                check(
                    &|out, rows| geometry.fill(pixels, &general, out, rows),
                    general.lanes(),
                    general_rows,
                );
                check(
                    &|out, rows| geometry.fill(pixels, &fused, out, rows),
                    fused.lanes(),
                    fused_rows,
                );
                check(
                    &|out, rows| geometry.fill(pixels, &opaque, out, rows),
                    5,
                    opaque_rows,
                );
            }
        }
    }
}

#[test]
fn opacity_tables_equal_scalar_opacities() {
    for bits in [1, 2, 4, 8] {
        let mut mask = image((16, 16), 1, bits, bytes(256, bits, 43));
        for decode in [[0.0, 1.0], [1.0, 0.0], [0.3, 0.6]] {
            let opacity = Opacity::new(&mask, decode, &mut meter()).unwrap();
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
            false,
            Transform::new(1e300, 0.0, 0.0, 1.0, 0.0, 0.0)
        )
        .is_err()
    );
    let mut empty = SpanColors::default();
    let sampler = ImageSampler::new(None, 3);
    sampler.span(Pixels::new(0, 0, 4), &mut scratch(&sampler), &mut empty);
    assert_eq!((empty.colors.len(), empty.alpha), (12, vec![0.0; 4]));
    let _ = gray;
}

#[test]
fn prepared_samplers_are_shared_between_threads() {
    fn shared<T: Send + Sync>() {}
    shared::<ImageSampler>();
}

#[test]
fn reused_paint_storage_drops_source_values_and_charges_the_same_layout() {
    use crate::source::Source;
    use pagura_core::Meter;

    let make = |seed| {
        let mut image = case(image((23, 17), 4, 8, bytes(23 * 17 * 4, 8, seed)));
        image.space = Some(ColorSpace::DeviceCmyk);
        Source::Image(sampler(&image, placements(23, 17)[0], 3))
    };
    let first = make(11);
    let second = make(17);
    let mut cold = Meter::new(usize::MAX, "test storage");
    let mut storage = PaintScratch::make(first.scratch_layout(), None, &mut cold).unwrap();
    assert!(storage.fits(second.scratch_layout()));
    let pixels = Pixels::new(0, 0, 20);
    let mut out = SpanColors::default();
    first.fill(pixels, &mut storage, &mut out, &mut Vec::new());
    let previous = bits(&out.colors);
    let mut reused = Meter::new(usize::MAX, "test storage");
    let mut storage =
        PaintScratch::make(second.scratch_layout(), Some(storage), &mut reused).unwrap();
    assert_eq!(cold.used(), reused.used());
    assert_eq!(
        storage.bytes(),
        second.scratch_layout().map_or(0, ScratchLayout::bytes)
    );
    second.fill(pixels, &mut storage, &mut out, &mut Vec::new());
    assert_ne!(previous, bits(&out.colors));
    let mut fresh = PaintScratch::make(second.scratch_layout(), None, &mut cold).unwrap();
    let mut expected = SpanColors::default();
    second.fill(pixels, &mut fresh, &mut expected, &mut Vec::new());
    assert_eq!(bits(&out.colors), bits(&expected.colors));
    let mut tight = Meter::new(
        second.scratch_layout().map_or(0, ScratchLayout::bytes) - 1,
        "test storage",
    );
    assert!(PaintScratch::make(second.scratch_layout(), Some(storage), &mut tight).is_err());
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

/// Source rows reduced separably, with texels from channel and Matte
/// tables, give every pixel the same bits for every partition of the device
/// row, within [`close`] of summing each pixel's per-sample texels over its
/// footprint, the image's and the mask's.
#[test]
#[expect(clippy::too_many_lines, reason = "one case table and its checks")]
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
                let mut storage = scratch(&sampler);
                let reserved = storage.bytes();
                let prepared = sampler.prepared.as_deref().unwrap();
                let super::Plan::Texels {
                    sampling: plan,
                    paint: source_paint,
                } = &prepared.plan
                else {
                    panic!("masked image samples converted texels");
                };
                let Paint::Colors(paint) = source_paint else {
                    panic!("colour image");
                };
                let (colors, _) = paint.as_ref();
                let mask = prepared.mask.as_ref().unwrap();
                let lanes = colors.lanes();
                let (x0, y0, x1, y1) = window(unit_to_device);
                let len = usize::try_from(x1 - x0).unwrap();
                let opacity = OpacityTexels {
                    mask: &mask.image.image,
                    opacity: &mask.opacity,
                };
                let matte = colors
                    .matte
                    .as_ref()
                    .map(|_| (&mask.image.image, &mask.opacity));
                let texels = SampleTexels::new(&prepared.image.image, colors, matte);
                // A mask on the image's grid averages ahead of the texels.
                let mut fused_values = Vec::new();
                let texels = super::texels::Fused {
                    texels: &texels,
                    mask: OpacityTexels {
                        mask: &mask.image.image,
                        opacity: &mask.opacity,
                    },
                    values: std::cell::RefCell::new(&mut fused_values),
                };
                assert!(mask.independent.is_none());
                assert!(storage.mask.is_none());
                let mask_plan =
                    super::Sampling::new(&prepared.geometry, (x0, x1), &mut meter()).unwrap();
                let mut mask_storage = mask_plan.layout(1).make().unwrap();
                let lanes = lanes + 1;
                for y in y0..y1 {
                    let mut whole = (vec![0.0; len * lanes], vec![0.0; len]);
                    prepared.texels(
                        (plan, source_paint),
                        Pixels::new(x0, y, len),
                        &mut storage,
                        (lanes - 1, TexelMode::Exact),
                        &mut whole.0,
                    );
                    assert_eq!(storage.bytes(), reserved);
                    mask_plan.fill(
                        &prepared.geometry,
                        Pixels::new(x0, y, len),
                        &mut mask_storage,
                        &opacity,
                        &mut whole.1,
                    );
                    for (x, count) in partitions(x0, len, y) {
                        let span = Pixels::new(x, y, count);
                        let start = usize::try_from(x - x0).unwrap();
                        let context = format!("{unit_to_device:?} target {target} span {span:?}");
                        let mut rows = vec![0.0; count * lanes];
                        prepared.texels(
                            (plan, source_paint),
                            span,
                            &mut storage,
                            (lanes - 1, TexelMode::Exact),
                            &mut rows,
                        );
                        let part = &whole.0[start * lanes..(start + count) * lanes];
                        assert_eq!(bits(&rows), bits(part), "{context}");
                        let mut samples = vec![0.0; count * lanes];
                        // A plan of its own: kept rows belong to one
                        // source of texels.
                        let own = super::Sampling::new(&prepared.geometry, (x0, x1), &mut meter())
                            .unwrap();
                        own.fill(
                            &prepared.geometry,
                            span,
                            &mut own.layout(lanes).make().unwrap(),
                            &texels,
                            &mut samples,
                        );
                        for (&row, &sample) in rows.iter().zip(&samples) {
                            assert!(close(row, sample), "{row} vs {sample} {context}");
                        }
                        let (mut rows, mut samples) = (vec![0.0; count], vec![0.0; count]);
                        mask_plan.fill(
                            &prepared.geometry,
                            span,
                            &mut mask_storage,
                            &opacity,
                            &mut rows,
                        );
                        let part = &whole.1[start..start + count];
                        assert_eq!(bits(&rows), bits(part), "mask {context}");
                        let own = super::Sampling::new(&prepared.geometry, (x0, x1), &mut meter())
                            .unwrap();
                        own.fill(
                            &prepared.geometry,
                            span,
                            &mut own.layout(1).make().unwrap(),
                            &opacity,
                            &mut samples,
                        );
                        for (&row, &sample) in rows.iter().zip(&samples) {
                            assert!(close(row, sample), "mask {row} vs {sample} {context}");
                        }
                    }
                }
            }
        }
    }
}

/// Wherever the sampler writes a span's words itself, they are the words
/// a store at full opacity writes for its colours.
#[test]
fn words_store_as_opaque_spans_do() {
    let rgb = case(image((9, 12), 3, 8, bytes(9 * 12 * 3, 8, 5)));
    let gray = case(image((13, 9), 1, 8, bytes(13 * 9, 8, 3)));
    let mut icc_like = case(image((10, 8), 3, 8, bytes(10 * 8 * 3, 8, 7)));
    icc_like.decode = vec![[0.1, 0.9], [0.0, 1.0], [1.0, 0.0]];
    // Lab samples convert through the memo, and upright nearest pixels
    // write their words as the memo's texel rows are read.
    let lab = pagura_color::cie::Calibrated::lab([0.9505, 1.0, 1.089], [0.0; 3], [-100.0; 4]);
    let lab = Case {
        space: Some(ColorSpace::Calibrated(Arc::new(lab.unwrap()))),
        decode: vec![[0.0, 100.0], [-100.0, 100.0], [-100.0, 100.0]],
        ..case(image((11, 9), 3, 8, bytes(11 * 9 * 3, 8, 13)))
    };
    let (mut written, mut averaged, mut memoized) = (0, 0, 0);
    for case in [&rgb, &gray, &icc_like, &lab] {
        for target in [3, 4] {
            for unit_to_device in placements(case.image.width, case.image.height) {
                let sampler = sampler(case, unit_to_device, target);
                let mode = sampler.prepared.as_ref().map(|p| p.geometry.mode());
                let memo = sampler.prepared.as_ref().is_some_and(|p| {
                    matches!(&p.plan, super::Plan::Texels { paint: Paint::Colors(paint), .. }
                        if matches!(paint.1, super::texels::Strategy::Memo(_)))
                });
                let mut storage = scratch(&sampler);
                let reserved = storage.bytes();
                let (x0, y0, x1, y1) = window(unit_to_device);
                let len = usize::try_from(x1 - x0).unwrap();
                let mut span = SpanColors::default();
                for y in y0..y1 {
                    // The whole row, and short spans that may lie inside the
                    // image, as averages need.
                    let short = (x0..x1).step_by(2).map(|x| (x, 3));
                    for (x, len) in std::iter::once((x0, len)).chain(short) {
                        let mut words = vec![[0u16; 4]; len];
                        if !sampler.words(Pixels::new(x, y, len), &mut storage, &mut words) {
                            continue;
                        }
                        written += 1;
                        averaged += usize::from(mode != Some(super::Mode::Nearest));
                        memoized += usize::from(memo && mode == Some(super::Mode::Nearest));
                        sampler.span(Pixels::new(x, y, len), &mut storage, &mut span);
                        assert_eq!(storage.bytes(), reserved);
                        assert!(span.alpha.iter().all(|&a| a == 1.0));
                        let mut expected = vec![[0u16; 4]; len];
                        crate::kernel::overwrite_n16(&mut expected, &span.colors);
                        assert_eq!(words, expected, "{unit_to_device:?} row {y} at {x}");
                    }
                }
            }
        }
    }
    assert!(written > 0 && averaged > 0 && memoized > 0);
}

/// A large source of few colours, indexed at preparation, samples bit for
/// bit as its memo does, and writes the same words: opaque and
/// colour-keyed, retained and replacing.
#[test]
fn indexed_sources_sample_as_their_memo_does() {
    let palette = [
        [0u8, 0, 0],
        [255, 255, 255],
        [200, 30, 90],
        [17, 140, 230],
        [90, 90, 91],
    ];
    let (width, height) = (64, 48);
    // Runs of one to four samples of one colour.
    let mut samples = Vec::new();
    for code in codes(width * height, 8, 9).into_iter().map(usize::from) {
        for _ in 0..=code % 4 {
            samples.extend(palette[code % 5]);
        }
    }
    samples.truncate(width * height * 3);
    let rgb = image(
        (
            u32::try_from(width).unwrap(),
            u32::try_from(height).unwrap(),
        ),
        3,
        8,
        Samples::Bytes(samples),
    );
    let mut keyed = case(rgb.clone());
    keyed.key = Some(vec![[150, 255], [0, 40], [0, 255]]);
    let lab = pagura_color::cie::Calibrated::lab([0.9505, 1.0, 1.089], [0.0; 3], [-100.0; 4]);
    let lab = Case {
        space: Some(ColorSpace::Calibrated(Arc::new(lab.unwrap()))),
        decode: vec![[0.0, 100.0], [-100.0, 100.0], [-100.0, 100.0]],
        ..case(rgb.clone())
    };
    let (mut indexed, mut written) = (0, 0);
    for (case, target) in [(&case(rgb), 4), (&keyed, 3), (&lab, 3)] {
        for unit_to_device in placements(case.image.width, case.image.height) {
            let fast = sampler(case, unit_to_device, target);
            let mut memo = sampler(case, unit_to_device, target);
            // The same source through its memo, before capacity discovery.
            let prepared = memo.prepared.as_deref_mut().unwrap();
            if let super::Plan::Texels {
                paint: Paint::Colors(paint),
                ..
            } = &mut prepared.plan
            {
                assert!(matches!(paint.1, super::texels::Strategy::Indexed(..)));
                indexed += 1;
                let image = &prepared.image.image;
                paint.1 = super::texels::Strategy::new(
                    image,
                    &paint.0,
                    false,
                    None,
                    &mut meter(),
                    &mut meter(),
                )
                .unwrap();
                assert!(matches!(paint.1, super::texels::Strategy::Memo(_)));
            }
            let (mut fast_storage, mut memo_storage) = (scratch(&fast), scratch(&memo));
            let (x0, y0, x1, y1) = window(unit_to_device);
            let len = usize::try_from(x1 - x0).unwrap();
            let (mut a, mut b) = (SpanColors::default(), SpanColors::default());
            let (mut a_shape, mut b_shape) = (Vec::new(), Vec::new());
            for y in y0..y1 {
                let pixels = Pixels::new(x0, y, len);
                fast.span_with_shape(pixels, &mut fast_storage, &mut a, &mut a_shape);
                memo.span_with_shape(pixels, &mut memo_storage, &mut b, &mut b_shape);
                let context = || format!("{unit_to_device:?} target {target} row {y}");
                assert_eq!(bits(&a.colors), bits(&b.colors), "{}", context());
                assert_eq!(bits(&a.alpha), bits(&b.alpha), "{}", context());
                assert_eq!(bits(&a_shape), bits(&b_shape), "{}", context());
                let short = (x0..x1).step_by(5).map(|x| (x, 3));
                for (x, len) in std::iter::once((x0, len)).chain(short) {
                    let pixels = Pixels::new(x, y, len);
                    let (mut a, mut b) = (vec![[0u16; 4]; len], vec![[0u16; 4]; len]);
                    let wrote = fast.words(pixels, &mut fast_storage, &mut a);
                    assert_eq!(wrote, memo.words(pixels, &mut memo_storage, &mut b));
                    assert_eq!(a, b, "words of {}", context());
                    written += usize::from(wrote);
                }
            }
        }
    }
    assert!(indexed > 0 && written > 0);
}

/// An ICCBased RGB space through the sampled, sRGB-like profile of the
/// colour fixtures: its conversion ends in the sRGB transfer function's
/// power.
fn icc_rgb() -> ColorSpace {
    let bytes = pagura_testkit::fixture("color", "sampled-rgb.icc");
    let profile = pagura_color::Profile::parse(&bytes, 3, &pagura_core::Limits::default())
        .unwrap()
        .unwrap();
    ColorSpace::Icc(Arc::new(pagura_color::IccSpace {
        profile: Some(Arc::new(profile)),
        alternate: Arc::new(ColorSpace::DeviceRgb),
        components: 3,
        range: [[0.0, 1.0]; 4],
        object: None,
    }))
}

/// Texels converted from each component's first-stage values, found as
/// texels need them, are those converted directly, exactly and kept to F32
/// levels, under every placement.
#[test]
fn curved_texels_are_direct_texels() {
    let photo = Case {
        space: Some(icc_rgb()),
        ..case(image((37, 29), 3, 8, bytes(37 * 29 * 3, 8, 97)))
    };
    let mut curved = 0;
    for unit_to_device in placements(37, 29) {
        let tabled = sampler(&photo, unit_to_device, 3);
        let mut direct = sampler(&photo, unit_to_device, 3);
        if let super::Plan::Texels {
            paint: Paint::Colors(paint),
            ..
        } = &mut direct.prepared.as_deref_mut().unwrap().plan
        {
            curved += usize::from(paint.0.curved.take().is_some());
        }
        let (mut tabled_storage, mut direct_storage) = (scratch(&tabled), scratch(&direct));
        let (x0, y0, x1, y1) = window(unit_to_device);
        let len = usize::try_from(x1 - x0).unwrap();
        let (mut a, mut b) = (SpanColors::default(), SpanColors::default());
        for y in y0..y1 {
            let exact = Pixels::new(x0, y, len);
            let levels = Pixels {
                precision: crate::source::Precision::Level,
                ..exact
            };
            for pixels in [exact, levels] {
                tabled.span(pixels, &mut tabled_storage, &mut a);
                direct.span(pixels, &mut direct_storage, &mut b);
                let place = format!("{unit_to_device:?} row {y} {:?}", pixels.precision);
                assert_eq!(bits(&a.colors), bits(&b.colors), "{place}");
                assert_eq!(bits(&a.alpha), bits(&b.alpha), "{place}");
            }
        }
    }
    assert!(curved > 0, "no image tabulated its first stage");
}

/// The curve values are optional: an image whose budget lacks room for them
/// keeps none and charges nothing for them, and its spans are those of an
/// image that keeps them.
#[test]
fn curve_values_are_declined_without_room() {
    let photo = Case {
        space: Some(icc_rgb()),
        ..case(image((37, 29), 3, 8, bytes(37 * 29 * 3, 8, 97)))
    };
    let curved = |sampler: &ImageSampler| match &sampler.prepared.as_deref().unwrap().plan {
        super::Plan::Texels {
            paint: Paint::Colors(paint),
            ..
        } => paint.0.curved.is_some(),
        _ => false,
    };
    let room = super::texels::Curved::bytes_of(3);
    let mut kept_any = false;
    for unit_to_device in placements(37, 29) {
        let mut kept = prepared_sampler(&photo, unit_to_device, 3);
        let mut declined = prepared_sampler(&photo, unit_to_device, 3);
        let bytes = declined.bytes();
        let mut short = pagura_core::Meter::new(room - 1, "test");
        declined.admit_curves(&mut short);
        assert!(!curved(&declined), "{unit_to_device:?}");
        assert_eq!(
            (declined.bytes(), short.used()),
            (bytes, 0),
            "{unit_to_device:?}"
        );
        let mut enough = pagura_core::Meter::new(room, "test");
        kept.admit_curves(&mut enough);
        if curved(&kept) {
            kept_any = true;
            assert_eq!(kept.bytes(), bytes + room, "{unit_to_device:?}");
            assert_eq!(enough.used(), room, "{unit_to_device:?}");
        }
        let (mut kept_storage, mut declined_storage) = (scratch(&kept), scratch(&declined));
        let (x0, y0, x1, y1) = window(unit_to_device);
        let len = usize::try_from(x1 - x0).unwrap();
        let (mut a, mut b) = (SpanColors::default(), SpanColors::default());
        for y in y0..y1 {
            let exact = Pixels::new(x0, y, len);
            let levels = Pixels {
                precision: crate::source::Precision::Level,
                ..exact
            };
            for pixels in [exact, levels] {
                kept.span(pixels, &mut kept_storage, &mut a);
                declined.span(pixels, &mut declined_storage, &mut b);
                let place = format!("{unit_to_device:?} row {y} {:?}", pixels.precision);
                assert_eq!(bits(&a.colors), bits(&b.colors), "{place}");
                assert_eq!(bits(&a.alpha), bits(&b.alpha), "{place}");
            }
        }
    }
    assert!(kept_any, "no image kept its curve values");
}

/// How `sampler` converts the texels of a span over `pixels`, and of its
/// words.
fn modes(sampler: &ImageSampler, pixels: Pixels<'_>) -> (TexelMode, TexelMode) {
    let prepared = sampler.prepared.as_deref().unwrap();
    match &prepared.plan {
        super::Plan::Texels { paint, .. } => (
            prepared.texel_mode(paint, pixels),
            prepared.word_mode(paint),
        ),
        super::Plan::Codes(_) => (TexelMode::Exact, TexelMode::Exact),
    }
}

/// Spans kept to F32 levels that copy one opaque texel per pixel convert
/// texels to values of the same levels: their levels and alphas are those
/// of exact spans, under every placement, with and without a soft mask
/// (which keeps texels exact).
#[test]
fn rounded_texels_keep_the_levels_of_exact_texels() {
    let level = |values: &[f64]| {
        values
            .iter()
            .map(|&v| crate::kernel::level(v).to_bits())
            .collect::<Vec<_>>()
    };
    let mut rounded = 0;
    // The smaller image converts its texels directly, the larger through its
    // components' first-stage values and their RGB conversion.
    for (width, height) in [(29, 19), (37, 29)] {
        let samples = usize::try_from(width * height).unwrap();
        let photo = Case {
            space: Some(icc_rgb()),
            ..case(image((width, height), 3, 8, bytes(samples * 3, 8, 83)))
        };
        let soft = Case {
            mask: Some((
                image((width, height), 1, 8, bytes(samples, 8, 89)),
                [0.0, 1.0],
                MaskUse::Soft(None),
            )),
            ..photo.clone()
        };
        for case in [&photo, &soft] {
            for unit_to_device in placements(width, height) {
                let sampler = sampler(case, unit_to_device, 3);
                let (mut exact_storage, mut kept_storage) = (scratch(&sampler), scratch(&sampler));
                let (x0, y0, x1, y1) = window(unit_to_device);
                let len = usize::try_from(x1 - x0).unwrap();
                let (mut exact, mut kept) = (SpanColors::default(), SpanColors::default());
                for y in y0..y1 {
                    let pixels = Pixels::new(x0, y, len);
                    let levels = Pixels {
                        precision: crate::source::Precision::Level,
                        ..pixels
                    };
                    sampler.span(pixels, &mut exact_storage, &mut exact);
                    sampler.span(levels, &mut kept_storage, &mut kept);
                    let place = format!("{width}x{height} {unit_to_device:?} row {y}");
                    assert_eq!(level(&kept.colors), level(&exact.colors), "{place}");
                    assert_eq!(bits(&kept.alpha), bits(&exact.alpha), "{place}");
                    rounded += usize::from(modes(&sampler, levels).0 == TexelMode::Rounded);
                }
            }
        }
    }
    assert!(rounded > 20, "{rounded} rounded spans");
}

/// Words of nearest samples of a memoized image convert texels to values of
/// the same N16 words: they are the words of exact spans under every
/// placement, and exact spans read through the same memo between them keep
/// their exact colours.
#[test]
fn word_texels_keep_the_words_of_exact_texels() {
    let word = |color: &[f64; 3]| {
        [
            crate::kernel::opaque_word(color[0]),
            crate::kernel::opaque_word(color[1]),
            crate::kernel::opaque_word(color[2]),
            u16::MAX,
        ]
    };
    let mut worded = 0;
    // As for levels: one image converts directly, the other through its
    // components' first-stage values and their RGB conversion.
    for (width, height) in [(29, 19), (37, 29)] {
        let samples = usize::try_from(width * height).unwrap();
        let photo = Case {
            space: Some(icc_rgb()),
            ..case(image((width, height), 3, 8, bytes(samples * 3, 8, 83)))
        };
        for unit_to_device in placements(width, height) {
            let sampler = sampler(&photo, unit_to_device, 3);
            let (mut exact_storage, mut storage) = (scratch(&sampler), scratch(&sampler));
            let (x0, y0, x1, y1) = window(unit_to_device);
            let len = usize::try_from(x1 - x0).unwrap();
            let (mut exact, mut between) = (SpanColors::default(), SpanColors::default());
            for y in y0..y1 {
                let pixels = Pixels::new(x0, y, len);
                let place = format!("{width}x{height} {unit_to_device:?} row {y}");
                sampler.span(pixels, &mut exact_storage, &mut exact);
                let mut words = vec![[0u16; 4]; len];
                if sampler.words(pixels, &mut storage, &mut words) {
                    let exact_words: Vec<_> =
                        exact.colors.as_chunks::<3>().0.iter().map(word).collect();
                    assert_eq!(words, exact_words, "{place}");
                    worded += usize::from(modes(&sampler, pixels).1 == TexelMode::Words);
                }
                sampler.span(pixels, &mut storage, &mut between);
                assert_eq!(bits(&between.colors), bits(&exact.colors), "{place}");
            }
        }
    }
    assert!(worded > 20, "{worded} spans of words");
}
