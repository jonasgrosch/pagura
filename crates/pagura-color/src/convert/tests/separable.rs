//! Conversions that split per component: a device colour in its target's
//! own family takes each channel from its own component
//! ([`Converter::per_channel`]), and DeviceRGB encodes its clipped
//! components ([`Converter::rgb_encoding`]), both bit for bit as
//! [`Converter::convert`] converts.

use super::*;
use crate::function::tests::random::Rng;

/// Component values in and around `0..=1`, the ends and their signs among
/// them.
fn value(rng: &mut Rng) -> f64 {
    const SPECIAL: [f64; 8] = [0.0, -0.0, 1.0, 0.5, -0.25, 1.25, 1e-300, 1.0 - f64::EPSILON];
    if rng.chance(20) {
        SPECIAL[rng.below(SPECIAL.len())]
    } else {
        rng.range(-0.5, 1.5)
    }
}

fn converter(space: &ColorSpace, target: &Target, conversion: &Conversion) -> Converter {
    Converter::new(space, target, conversion).unwrap()
}

#[test]
fn rgb_encodings_convert_as_their_converters_do() {
    let mut rng = Rng::new(0x5e_9a_ab);
    let mut encoded = 0;
    // Into RGB, DeviceRGB is the target's own family: per channel.
    for process in [Process::Gray, Process::Cmyk] {
        let target = Target::new(process);
        let converter = converter(&ColorSpace::DeviceRgb, &target, &Conversion::default());
        let encoding = converter.rgb_encoding().unwrap();
        let channels = target.channels();
        for _ in 0..20_000 {
            let rgb = [value(&mut rng), value(&mut rng), value(&mut rng)];
            let mut whole = [0.0; 4];
            converter.convert(&rgb, &mut whole[..channels]);
            let split = encoding.encode(rgb.map(RgbEncoding::clip));
            assert_eq!(
                bits(&whole[..channels]),
                bits(&split[..channels]),
                "{process:?} {rgb:?}"
            );
            encoded += 1;
        }
    }
    assert_eq!(encoded, 40_000);
}

/// Black generation or undercolour removal functions, spot channels and
/// spaces other than DeviceRGB have no RGB encoding.
#[test]
fn only_device_rgb_without_functions_or_spots_encodes() {
    let mut rng = Rng::new(0x0e_dc_0d);
    let cmyk = Target::new(Process::Cmyk);
    let (function, _) = tint_function(&mut rng, 1, 1);
    let black = Conversion {
        black_generation: Some(Arc::clone(&function)),
        ..Conversion::default()
    };
    let removal = Conversion {
        undercolor_removal: Some(function),
        ..Conversion::default()
    };
    for conversion in [&black, &removal] {
        assert!(
            converter(&ColorSpace::DeviceRgb, &cmyk, conversion)
                .rgb_encoding()
                .is_none()
        );
    }
    let spots = Target::separated(Process::Cmyk, vec![Name::new(b"SpotA")].into());
    let default = Conversion::default();
    assert!(
        converter(&ColorSpace::DeviceRgb, &spots, &default)
            .rgb_encoding()
            .is_none()
    );
    for space in [ColorSpace::DeviceGray, ColorSpace::DeviceCmyk] {
        assert!(converter(&space, &cmyk, &default).rgb_encoding().is_none());
    }
    let indexed = ColorSpace::Indexed(Arc::new(Indexed {
        base: Arc::new(ColorSpace::DeviceRgb),
        high: 1,
        lookup: vec![0, 0, 0, 255, 255, 255].into(),
    }));
    assert!(
        converter(&indexed, &cmyk, &default)
            .rgb_encoding()
            .is_none()
    );
}

#[test]
fn per_channel_conversions_take_each_channel_from_its_component() {
    let mut rng = Rng::new(0x9c_4a_77);
    let default = Conversion::default();
    for (space, process) in [
        (ColorSpace::DeviceCmyk, Process::Cmyk),
        (ColorSpace::DeviceRgb, Process::Rgb),
        (ColorSpace::DeviceGray, Process::Gray),
    ] {
        let target = Target::new(process);
        let converter = converter(&space, &target, &default);
        assert!(converter.per_channel(), "{space:?}");
        let n = space.components();
        for _ in 0..20_000 {
            let components: Vec<f64> = (0..n).map(|_| value(&mut rng)).collect();
            let mut whole = [0.0; 4];
            converter.convert(&components, &mut whole[..n]);
            for (i, &component) in components.iter().enumerate() {
                let mut alone = [0.0; 4];
                converter.convert(&vec![component; n], &mut alone[..n]);
                assert_eq!(
                    whole[i].to_bits(),
                    alone[i].to_bits(),
                    "{space:?} channel {i} of {components:?}"
                );
            }
        }
    }
    let spots = Target::separated(Process::Cmyk, vec![Name::new(b"SpotA")].into());
    assert!(!converter(&ColorSpace::DeviceCmyk, &spots, &default).per_channel());
    let cmyk = Target::new(Process::Cmyk);
    for space in [ColorSpace::DeviceRgb, ColorSpace::DeviceGray] {
        assert!(
            !converter(&space, &cmyk, &default).per_channel(),
            "{space:?}"
        );
    }
    let rgb = Target::new(Process::Rgb);
    assert!(!converter(&ColorSpace::DeviceCmyk, &rgb, &default).per_channel());
}
