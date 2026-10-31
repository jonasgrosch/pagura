//! Photographic samples whose conversion splits per component sample
//! through tables ([`Strategy::Cmyk`], [`Strategy::RgbEncoded`]) to their
//! memo's texels, bit for bit; other samples keep their memo or palette.

use super::*;
use crate::source::image::texels::Strategy;
use pagura_color::{ExponentialFunction, Function};

/// The strategy `sampler`'s colours took, for changing it.
fn strategy(sampler: &mut ImageSampler) -> &mut Strategy {
    match &mut sampler.prepared.as_deref_mut().unwrap().plan {
        super::super::Plan::Texels {
            paint: Paint::Colors(paint),
            ..
        } => &mut paint.1,
        _ => panic!("no colours"),
    }
}

/// The same sampler with its colours' memo in place of its tables.
fn through_memo(mut sampler: ImageSampler) -> ImageSampler {
    let prepared = sampler.prepared.as_deref_mut().unwrap();
    let image = &prepared.image.image;
    if let super::super::Plan::Texels {
        paint: Paint::Colors(paint),
        ..
    } = &mut prepared.plan
    {
        paint.1 = Strategy::memo(image, &paint.0, None).unwrap();
    }
    sampler
}

/// Photographic DeviceRGB and DeviceCMYK 8-bit samples (too many colours
/// for a palette) into a CMYK target, and DeviceRGB into a gray one, take
/// their tables and sample as their memo does: every colour, alpha and shape, bit for bit, over whole rows and
/// short spans. Decode arrays that invert and that reach past `0..1`
/// exercise the conversion's clipping, and interpolated samples the
/// weighted sums.
#[test]
fn separable_samples_sample_as_their_memo_does() {
    let rgb = image((61, 53), 3, 8, bytes(61 * 53 * 3, 8, 73));
    let cmyk = image((59, 51), 4, 8, bytes(59 * 51 * 4, 8, 79));
    let mut cases = Vec::new();
    for decode in [
        vec![[0.0, 1.0]; 3],
        vec![[1.0, 0.0], [-0.25, 1.25], [0.1, 0.9]],
    ] {
        for target in [4, 1] {
            cases.push((
                Case {
                    decode: decode.clone(),
                    ..case(rgb.clone())
                },
                target,
            ));
        }
    }
    // Interpolated samples add weighted texels.
    let mut smooth = rgb.clone();
    smooth.interpolate = true;
    cases.push((case(smooth), 4));
    for decode in [
        vec![[0.0, 1.0]; 4],
        vec![[1.0, 0.0], [-0.5, 1.5], [0.0, 1.0], [0.2, 0.7]],
    ] {
        cases.push((
            Case {
                space: Some(ColorSpace::DeviceCmyk),
                decode,
                ..case(cmyk.clone())
            },
            4,
        ));
    }
    let mut spans = 0;
    for (case, target) in &cases {
        for unit_to_device in placements(case.image.width, case.image.height) {
            let mut tables = sampler(case, unit_to_device, *target);
            assert!(
                matches!(
                    strategy(&mut tables),
                    Strategy::Cmyk(_) | Strategy::RgbEncoded(_)
                ),
                "{unit_to_device:?} target {target}"
            );
            let memo = through_memo(sampler(case, unit_to_device, *target));
            let (mut table_storage, mut memo_storage) = (scratch(&tables), scratch(&memo));
            let (x0, y0, x1, y1) = window(unit_to_device);
            let len = usize::try_from(x1 - x0).unwrap();
            let (mut a, mut b) = (SpanColors::default(), SpanColors::default());
            let (mut a_shape, mut b_shape) = (Vec::new(), Vec::new());
            for y in y0..y1 {
                let short = (x0..x1).step_by(11).map(|x| (x, 3.min(x1 - x)));
                for (x, n) in std::iter::once((x0, x1 - x0)).chain(short) {
                    let pixels = Pixels::new(x, y, usize::try_from(n).unwrap());
                    tables.span_with_shape(pixels, &mut table_storage, &mut a, &mut a_shape);
                    memo.span_with_shape(pixels, &mut memo_storage, &mut b, &mut b_shape);
                    let context = || format!("{unit_to_device:?} target {target} ({x}, {y})");
                    assert_eq!(bits(&a.colors), bits(&b.colors), "{}", context());
                    assert_eq!(bits(&a.alpha), bits(&b.alpha), "{}", context());
                    assert_eq!(bits(&a_shape), bits(&b_shape), "{}", context());
                    spans += 1;
                }
            }
            assert!(len > 0);
        }
    }
    assert!(spans > 1000, "{spans} spans");
}

/// A function of one input that is not the identity, for black
/// generation and undercolour removal.
fn curve() -> Arc<Function> {
    Arc::new(
        Function::exponential(ExponentialFunction {
            domain: [0.0, 1.0],
            range: None,
            c0: vec![0.0],
            c1: vec![1.0],
            exponent: 2.0,
        })
        .unwrap(),
    )
}

/// Samples the tables cannot serve keep their memo: black generation or
/// undercolour removal functions, a colour key, a Matte, embedded opacity.
/// Indexed lookups and sources few enough in colours for a palette keep
/// their table or palette. Plain photographic samples take tables.
#[test]
fn other_samples_keep_their_memo() {
    let placement = Transform::new(26.0, 0.0, 0.0, 22.0, 1.5, 2.5);
    let is_table =
        |strategy: &Strategy| matches!(strategy, Strategy::Cmyk(_) | Strategy::RgbEncoded(_));
    let is_memo = |strategy: &Strategy| matches!(strategy, Strategy::Memo(_));
    let (width, height) = (61, 53);
    let count = usize::try_from(width * height).unwrap();
    let rgb = case(image((width, height), 3, 8, bytes(count * 3, 8, 83)));
    let cmyk = Case {
        space: Some(ColorSpace::DeviceCmyk),
        decode: vec![[0.0, 1.0]; 4],
        ..case(image((width, height), 4, 8, bytes(count * 4, 8, 89)))
    };
    for plain in [&rgb, &cmyk] {
        assert!(is_table(strategy(&mut sampler(plain, placement, 4))));
    }
    // Black generation or undercolour removal functions.
    let (mut black, mut removal) = (Conversion::default(), Conversion::default());
    black.black_generation = Some(curve());
    removal.undercolor_removal = Some(curve());
    let mut sampler_rgb = sampler(&rgb, placement, 4);
    let prepared = sampler_rgb.prepared.as_deref_mut().unwrap();
    let decoded = &prepared.image.image;
    if let super::super::Plan::Texels {
        paint: Paint::Colors(paint),
        ..
    } = &mut prepared.plan
    {
        let target = Target::new(Process::Cmyk);
        for (conversion, tables) in [
            (Conversion::default(), true),
            (black, false),
            (removal, false),
        ] {
            paint.0.convert = Converter::new(&ColorSpace::DeviceRgb, &target, &conversion).unwrap();
            let separable =
                Strategy::separable(decoded, &paint.0, &mut meter(), &mut meter()).unwrap();
            assert_eq!(separable.is_some(), tables);
        }
    }
    // A colour key.
    for plain in [&rgb, &cmyk] {
        let keyed = Case {
            key: Some(vec![[0, 128]; usize::from(plain.image.components)]),
            ..plain.clone()
        };
        assert!(is_memo(strategy(&mut sampler(&keyed, placement, 4))));
    }
    // A Matte.
    let matte = Case {
        mask: Some((
            image_mask((width, height)),
            [0.0, 1.0],
            MaskUse::Soft(Some(vec![0.5; 3])),
        )),
        ..rgb.clone()
    };
    assert!(is_memo(strategy(&mut sampler(&matte, placement, 4))));
    // Embedded opacity.
    let mut alpha = rgb.clone();
    alpha.image.mask = MaskKind::Embedded(ImageAlpha {
        bits_per_component: 8,
        samples: bytes(count, 8, 97),
        premultiplied: false,
    });
    assert!(is_memo(strategy(&mut sampler(&alpha, placement, 4))));
    // An Indexed lookup into DeviceRGB.
    let lookup: Vec<u8> = (0..=255u8).flat_map(|v| [v, 255 - v, v / 3]).collect();
    let indexed = Case {
        space: Some(ColorSpace::Indexed(Arc::new(Indexed {
            base: Arc::new(ColorSpace::DeviceRgb),
            high: 255,
            lookup: lookup.into(),
        }))),
        decode: vec![[0.0, 255.0]],
        ..case(image((width, height), 1, 8, bytes(count, 8, 101)))
    };
    assert!(!is_table(strategy(&mut sampler(&indexed, placement, 4))));
    // A source of few colours keeps its palette.
    let palette = [[0u8, 0, 0], [255, 255, 255], [200, 30, 90], [17, 140, 230]];
    let samples = codes(count, 8, 103)
        .into_iter()
        .flat_map(|code| palette[usize::from(code) % 4])
        .collect();
    let few = case(image((width, height), 3, 8, Samples::Bytes(samples)));
    assert!(matches!(
        strategy(&mut sampler(&few, placement, 4)),
        Strategy::Indexed(..)
    ));
}
