//! Images whose memo holds bounds texels as well
//! ([`TexelMode::Bounds`]): their spans keep the F32 levels and N16 words
//! of exact spans, a pixel certified by its bounds or exact by itself, and
//! images the bounds cannot serve keep their storage.

use super::super::UNCERTAIN;
use super::*;
use crate::source::Precision;

/// An ICC RGB photograph of `size`: too many colours for a palette.
fn photo((width, height): (u32, u32), seed: u32) -> Case {
    let count = usize::try_from(width * height).unwrap() * 3;
    Case {
        space: Some(icc_rgb()),
        ..case(image((width, height), 3, 8, bytes(count, 8, seed)))
    }
}

/// Placements that average area footprints: minified, mirrored, and near
/// 1:1.
fn averaging(width: u32, height: u32) -> Vec<Transform> {
    let (w, h) = (f64::from(width), f64::from(height));
    vec![
        Transform::new(w / 2.4, 0.0, 0.0, -h / 3.2, 1.25, 1.0 + h / 3.2),
        Transform::new(w / 7.3, 0.0, 0.0, h / 5.1, 0.25, 0.5),
        Transform::new(-w / 3.0, 0.0, 0.0, h / 2.0, 0.5 + w / 3.0, 0.75),
        Transform::new(w * 0.93, 0.0, 0.0, h * 0.97, 0.5, 0.25),
    ]
}

/// `case`'s sampler under `unit_to_device` into RGB with its memo widened
/// for bounds texels; `None` where the bounds cannot serve it.
fn bounded(case: &Case, unit_to_device: Transform) -> Option<ImageSampler> {
    let mut sampler = sampler(case, unit_to_device, 3);
    sampler.bounds_layout()?;
    sampler.keep_bounds();
    Some(sampler)
}

/// The F32 levels of `values` ([`crate::kernel::level`]).
fn levels(values: &[f64]) -> Vec<u32> {
    values
        .iter()
        .map(|&v| crate::kernel::level(v).to_bits())
        .collect()
}

/// Compares the spans of `bounds` with those of `exact` over the rows of
/// `unit_to_device`'s window, whole and in short spans: exact spans bit
/// for bit, spans kept to F32 levels by their levels (or bit for bit
/// where `bitwise`), alpha and shape bit for bit, and N16 words. Returns
/// the spans compared.
fn compare(
    (bounds, exact): (&ImageSampler, &ImageSampler),
    unit_to_device: Transform,
    bitwise: bool,
) -> usize {
    let (mut kept, mut base) = (scratch(bounds), scratch(exact));
    let (x0, y0, x1, y1) = window(unit_to_device);
    let (mut a, mut b) = (SpanColors::default(), SpanColors::default());
    let (mut a_shape, mut b_shape) = (Vec::new(), Vec::new());
    let mut spans = 0;
    for y in y0..y1 {
        let short = (x0..x1).step_by(7).map(|x| (x, (x1 - x).min(3)));
        for (x, len) in std::iter::once((x0, x1 - x0)).chain(short) {
            let len = usize::try_from(len).unwrap();
            let place = format!("{unit_to_device:?} ({x}, {y})");
            let pixels = Pixels::new(x, y, len);
            let level = Pixels {
                precision: Precision::Level,
                ..pixels
            };
            for pixels in [pixels, level] {
                bounds.span_with_shape(pixels, &mut kept, &mut a, &mut a_shape);
                exact.span_with_shape(pixels, &mut base, &mut b, &mut b_shape);
                if pixels.precision == Precision::Level && !bitwise {
                    assert_eq!(levels(&a.colors), levels(&b.colors), "{place}");
                } else {
                    assert_eq!(bits(&a.colors), bits(&b.colors), "{place} exact");
                }
                assert_eq!(bits(&a.alpha), bits(&b.alpha), "{place}");
                assert_eq!(bits(&a_shape), bits(&b_shape), "{place}");
            }
            let (mut a_words, mut b_words) = (vec![[0u16; 4]; len], vec![[0u16; 4]; len]);
            let written = bounds.words(pixels, &mut kept, &mut a_words);
            assert_eq!(written, exact.words(pixels, &mut base, &mut b_words));
            assert_eq!(a_words, b_words, "{place} words");
            spans += 1;
        }
    }
    spans
}

/// Photographs averaged under minified, mirrored and near 1:1 placements
/// keep, with bounds texels, the F32 levels and N16 words of their exact
/// spans, and their exact spans bit for bit: exact texels share the
/// widened memo's slots with bounds texels.
#[test]
fn bounds_spans_keep_the_levels_and_words_of_exact_spans() {
    let (mut admitted, mut spans) = (0, 0);
    for case in [photo((61, 53), 101), photo((97, 83), 103)] {
        for unit_to_device in averaging(case.image.width, case.image.height) {
            let Some(bounds) = bounded(&case, unit_to_device) else {
                continue;
            };
            let exact = sampler(&case, unit_to_device, 3);
            let (wide, narrow) = (bounds.scratch_layout(), exact.scratch_layout());
            assert_ne!(wide, narrow);
            assert_eq!(
                bounds.bounds_bytes(),
                wide.bytes().saturating_sub(narrow.bytes())
            );
            spans += compare((&bounds, &exact), unit_to_device, false);
            admitted += 1;
        }
    }
    assert!(
        admitted >= 6 && spans > 1000,
        "{admitted} admitted, {spans} spans"
    );
}

/// A pixel whose bounds are uncertain takes its exact colour, as a span of
/// that pixel alone gives it: with every pixel forced so, spans of bounds
/// texels are their exact spans bit for bit, at F32 levels too, under
/// minified and near 1:1 placements.
#[test]
fn uncertain_pixels_take_their_exact_colours() {
    UNCERTAIN.with(|uncertain| uncertain.set(true));
    let (mut admitted, mut near) = (0, 0);
    let case = photo((61, 53), 107);
    for unit_to_device in averaging(61, 53) {
        let Some(bounds) = bounded(&case, unit_to_device) else {
            continue;
        };
        let exact = sampler(&case, unit_to_device, 3);
        compare((&bounds, &exact), unit_to_device, true);
        admitted += 1;
        near += usize::from(unit_to_device.a > 0.9 * 61.0);
    }
    UNCERTAIN.with(|uncertain| uncertain.set(false));
    assert!(
        admitted >= 3 && near == 1,
        "{admitted} admitted, {near} near 1:1"
    );
}

/// Bounds serve area averages of unmasked opaque RGB images placed
/// separably, whose memo retains few keys and whose curved values convert
/// through an RGB step, and nothing else: magnified, rotated or skewed
/// placements, other targets, curve values not admitted, masks, colour
/// keys and embedded opacity, device RGB, a photograph whose memo retains
/// more keys than [`MemoLayout::BOUNDED`], and a memo that holds bounds
/// already.
#[test]
fn bounds_serve_only_what_they_can_certify() {
    let (w, h) = (61.0, 53.0);
    let minified = Transform::new(w / 2.4, 0.0, 0.0, h / 3.2, 1.25, 1.0);
    let plain = photo((61, 53), 109);
    assert!(sampler(&plain, minified, 3).bounds_layout().is_some());
    for unit_to_device in [
        Transform::new(3.7 * w, 0.0, 0.0, 3.3 * h, 0.4, 0.3),
        Transform::new(w, 0.0, 0.0, h, 0.0, 0.0),
        Transform::new(w * 0.31, w * 0.17, -h * 0.13, h * 0.29, 4.5, 0.25),
    ] {
        assert!(
            sampler(&plain, unit_to_device, 3).bounds_layout().is_none(),
            "{unit_to_device:?}"
        );
    }
    for target in [1, 4] {
        assert!(sampler(&plain, minified, target).bounds_layout().is_none());
    }
    assert!(
        prepared_sampler(&plain, minified, 3)
            .bounds_layout()
            .is_none()
    );
    let count = 61 * 53;
    let masked = Case {
        mask: Some((image_mask((61, 53)), [0.0, 1.0], MaskUse::Soft(None))),
        ..plain.clone()
    };
    let keyed = Case {
        key: Some(vec![[0, 128]; 3]),
        ..plain.clone()
    };
    let mut alpha = plain.clone();
    alpha.image.mask = MaskKind::Embedded(ImageAlpha {
        bits_per_component: 8,
        samples: bytes(count, 8, 113),
        premultiplied: false,
    });
    let device = Case {
        space: Some(ColorSpace::DeviceRgb),
        ..plain.clone()
    };
    for case in [&masked, &keyed, &alpha, &device] {
        assert!(sampler(case, minified, 3).bounds_layout().is_none());
    }
    // A photograph of more colours than the memo retains converts each
    // about once.
    let large = photo((900, 700), 127);
    let small = Transform::new(900.0 / 4.5, 0.0, 0.0, 700.0 / 3.5, 0.5, 0.5);
    assert!(sampler(&large, small, 3).bounds_layout().is_none());
    let mut kept = bounded(&plain, minified).unwrap();
    assert!(kept.bounds_layout().is_none());
    kept.keep_bounds();
    assert_eq!(
        kept.scratch_layout(),
        bounded(&plain, minified).unwrap().scratch_layout()
    );
}
