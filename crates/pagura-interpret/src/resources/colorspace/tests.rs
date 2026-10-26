use super::*;
use pagura_document::OpenOptions;

fn document() -> Document {
    Document::open(
        pagura_testkit::fixture("classic", "rectangle.pdf"),
        &OpenOptions::default(),
    )
    .unwrap()
}

fn resources(spaces: &[(&[u8], Value)]) -> Dictionary {
    let mut spaces_dictionary = Dictionary::new();
    for (name, value) in spaces {
        spaces_dictionary.insert(Name::new(name), value.clone());
    }
    let mut resources = Dictionary::new();
    resources.insert(
        Name::new(b"ColorSpace"),
        Value::Dictionary(spaces_dictionary),
    );
    resources
}

#[test]
fn device_names_resolve_to_shared_spaces_unless_a_default_replaces_them() {
    let document = document();
    let mut colors = Colors::new(&document, None);
    let none = Dictionary::new();
    let gray = colors.named(&none, b"DeviceGray").unwrap();
    assert!(Arc::ptr_eq(&gray, &colors.device_gray()));
    let defaults = resources(&[(b"DefaultGray", Value::name(b"DeviceRGB"))]);
    let error = colors.named(&defaults, b"DeviceGray").unwrap_err();
    assert_eq!(
        error.message,
        "default colour space has wrong component count"
    );
    let lab = Value::Array(vec![
        Value::name(b"Lab"),
        Value::Dictionary({
            let mut d = Dictionary::new();
            d.insert(
                Name::new(b"WhitePoint"),
                Value::Array(vec![
                    Value::Real(0.95),
                    Value::Integer(1),
                    Value::Real(1.09),
                ]),
            );
            d
        }),
    ]);
    let defaults = resources(&[(b"DefaultRGB", lab)]);
    let error = colors.named(&defaults, b"DeviceRGB").unwrap_err();
    assert_eq!(error.message, "invalid default colour space family");
}

#[test]
fn indexed_and_pattern_spaces_keep_their_bases() {
    let document = document();
    let mut colors = Colors::new(&document, None);
    let indexed = Value::Array(vec![
        Value::name(b"Indexed"),
        Value::name(b"DeviceRGB"),
        Value::Integer(1),
        Value::String(vec![0, 0, 0, 255, 255, 255]),
    ]);
    let space = colors.resolve(&Dictionary::new(), &indexed).unwrap();
    let ColorSpace::Indexed(indexed) = space.as_ref() else {
        panic!("{space:?}")
    };
    assert_eq!((indexed.high, indexed.lookup.len()), (1, 6));
    let pattern = Value::Array(vec![Value::name(b"Pattern"), Value::name(b"DeviceGray")]);
    let space = colors.resolve(&Dictionary::new(), &pattern).unwrap();
    assert!(matches!(space.as_ref(), ColorSpace::Pattern(Some(base)) if base.components() == 1));
    let short = Value::Array(vec![
        Value::name(b"Indexed"),
        Value::name(b"DeviceRGB"),
        Value::Integer(1),
        Value::String(vec![0, 0, 0]),
    ]);
    let error = colors.resolve(&Dictionary::new(), &short).unwrap_err();
    assert_eq!(error.message, "Indexed lookup has wrong length");
}

#[test]
fn unknown_families_are_unsupported_and_devicen_names_are_checked() {
    let document = document();
    let mut colors = Colors::new(&document, None);
    let unknown = Value::Array(vec![Value::name(b"Spectral"), Value::Integer(3)]);
    let error = colors.resolve(&Dictionary::new(), &unknown).unwrap_err();
    assert_eq!(
        error.feature(),
        Some(pagura_color::features::UNKNOWN_FAMILY)
    );
    let all = Value::Array(vec![
        Value::name(b"DeviceN"),
        Value::Array(vec![Value::name(b"All")]),
        Value::name(b"DeviceGray"),
        Value::Null,
    ]);
    let error = colors.resolve(&Dictionary::new(), &all).unwrap_err();
    assert_eq!(error.message, "All is forbidden in DeviceN");
}

#[test]
fn embedded_icc_profiles_share_the_store_and_keep_errors_and_budgets() {
    use pagura_core::{ErrorKind, Limits, ObjectId};

    let document = document();
    let data = pagura_testkit::fixture("color", "sampled-rgb.icc");
    let id = ObjectId::new(4, 0);
    let store = Store::new(1 << 20);
    let mut colors = Colors::new(&document, Some(&store));
    let color = pagura_image::jpx::JpxColor::Icc(data.clone());
    let intent = pagura_color::Intent::Relative;
    let space = colors
        .embedded_jpx(id, &color, &[8; 3], intent, JpxSamples::Unsigned)
        .unwrap();
    let other = Colors::new(&document, Some(&store))
        .embedded_jpx(id, &color, &[8; 3], intent, JpxSamples::Unsigned)
        .unwrap();
    let (ColorSpace::Icc(first), ColorSpace::Icc(second)) = (space.as_ref(), other.as_ref()) else {
        panic!("embedded ICC did not produce an ICC space");
    };
    assert!(Arc::ptr_eq(
        first.profile.as_ref().unwrap(),
        second.profile.as_ref().unwrap()
    ));
    assert_eq!(first.object, Some(id));
    assert_eq!(first.range, [[0.0, 1.0]; 4]);
    assert!(matches!(
        colors
            .embedded_jpx(id, &color, &[8], intent, JpxSamples::Unsigned)
            .unwrap_err()
            .kind,
        ErrorKind::Syntax
    ));
    assert!(matches!(
        colors
            .embedded_jpx(
                ObjectId::new(5, 0),
                &pagura_image::jpx::JpxColor::Icc(b"broken".to_vec()),
                &[8; 3],
                intent,
                JpxSamples::Unsigned,
            )
            .unwrap_err()
            .kind,
        ErrorKind::Syntax
    ));

    for limits in [
        Limits {
            decode_work: 0,
            ..Limits::default()
        },
        Limits {
            decode_bytes: 0,
            ..Limits::default()
        },
        Limits {
            render_bytes: 0,
            ..Limits::default()
        },
    ] {
        let bounded = Document::open(
            pagura_testkit::fixture("classic", "rectangle.pdf"),
            &OpenOptions {
                limits,
                ..OpenOptions::default()
            },
        )
        .unwrap();
        let error = Colors::new(&bounded, None)
            .embedded_jpx(id, &color, &[8; 3], intent, JpxSamples::Unsigned)
            .unwrap_err();
        assert!(matches!(error.kind, ErrorKind::Limit), "{error}");
    }
}

#[test]
fn jpx_candidates_and_intents_do_not_alias_in_profile_caches() {
    use pagura_color::Intent;
    use pagura_core::ObjectId;
    use pagura_image::ImageSource;
    let document = Document::open(
        pagura_testkit::fixture("paint_jpx_integration", "selected-transform-relative.pdf"),
        &OpenOptions::default(),
    )
    .unwrap();
    let id = ObjectId::new(5, 0);
    let source = ImageSource::from_xobject(&document, document.object(id).unwrap()).unwrap();
    let pagura_image::ImageKind::EmbeddedColor { color, .. } = source.kind() else {
        panic!("missing colour choices")
    };
    let store = Store::new(1 << 20);
    for intent in [
        Intent::Relative,
        Intent::Perceptual,
        Intent::Relative,
        Intent::Absolute,
    ] {
        let mut colors = Colors::new(&document, Some(&store));
        let space = colors
            .embedded_jpx(id, color, &[8; 3], intent, JpxSamples::Unsigned)
            .unwrap();
        assert_eq!(
            matches!(space.as_ref(), ColorSpace::Icc(_)),
            intent == Intent::Perceptual
        );
    }
}

#[test]
fn enumerated_lab_uses_each_unsigned_channel_precision() {
    use pagura_color::Intent;
    use pagura_core::ObjectId;
    use pagura_image::jpx::{JpxColor, LabParameters};
    let document = document();
    let mut colors = Colors::new(&document, None);
    let default = JpxColor::Enumerated(14);
    let explicit = JpxColor::Lab(LabParameters::new(
        [200, 120, 240],
        [64, 96, 160],
        0x0044_3530,
    ));
    // M-18 endpoints, including fractional b offsets at one/two bits and
    // the 8/12/16-bit palette's output precisions, not its index precision.
    for (color, bits, expected) in [
        (
            &default,
            [1; 3],
            [[0.0, 100.0], [-170.0, 0.0], [-150.0, 50.0]],
        ),
        (
            &default,
            [2; 3],
            [[0.0, 100.0], [-340.0 / 3.0, 170.0 / 3.0], [-100.0, 100.0]],
        ),
        (
            &default,
            [8, 12, 16],
            [
                [0.0, 100.0],
                [-348_160.0 / 4095.0, 347_990.0 / 4095.0],
                [-4_915_200.0 / 65_535.0, 8_191_800.0 / 65_535.0],
            ],
        ),
        (
            &explicit,
            [8; 3],
            [
                [-12_800.0 / 255.0, 38_200.0 / 255.0],
                [-11_520.0 / 255.0, 19_080.0 / 255.0],
                [-38_400.0 / 255.0, 22_800.0 / 255.0],
            ],
        ),
    ] {
        let space = colors
            .embedded_jpx(
                ObjectId::new(5, 0),
                color,
                &bits,
                Intent::Relative,
                JpxSamples::Unsigned,
            )
            .unwrap();
        let ColorSpace::Calibrated(lab) = space.as_ref() else {
            panic!("expected CIELab")
        };
        for (actual, expected) in lab
            .lab_ranges()
            .unwrap()
            .into_iter()
            .flatten()
            .zip(expected.into_iter().flatten())
        {
            assert!((actual - expected).abs() < 1e-12);
        }
    }
}

#[test]
fn unavailable_lab_candidates_do_not_assume_device_rgb() {
    use pagura_color::Intent;
    use pagura_core::{ErrorKind, ObjectId};
    use pagura_image::jpx::{JpxColor, LabParameters};
    let document = document();
    let mut colors = Colors::new(&document, None);
    let id = ObjectId::new(5, 0);
    for color in [
        JpxColor::Lab(LabParameters::new(
            [100, 170, 200],
            [0, 128, 96],
            0x0044_3635,
        )),
        JpxColor::Lab(LabParameters::new([0, 170, 200], [0, 128, 96], 0x0044_3530)),
        JpxColor::Lab(LabParameters::new(
            [100, 170, 200],
            [0, 128, 96],
            0x4354_1d4c,
        )),
        JpxColor::Lab(LabParameters::new(
            [100, 170, 200],
            [0, 128, 96],
            0x5a5a_5a5a,
        )),
    ] {
        assert!(matches!(
            colors
                .embedded_jpx(id, &color, &[8; 3], Intent::Relative, JpxSamples::Unsigned)
                .unwrap_err()
                .kind,
            ErrorKind::Unsupported(_)
        ));
    }
    let color = JpxColor::Enumerated(14);
    assert!(matches!(
        colors
            .embedded_jpx(id, &color, &[8; 3], Intent::Relative, JpxSamples::Signed)
            .unwrap_err()
            .kind,
        ErrorKind::Unsupported(_)
    ));
    for bits in [[0, 8, 8], [8, 17, 8]] {
        assert!(matches!(
            colors
                .embedded_jpx(id, &color, &bits, Intent::Relative, JpxSamples::Unsigned)
                .unwrap_err()
                .kind,
            ErrorKind::Unsupported(_)
        ));
    }
    assert_eq!(
        colors
            .embedded_jpx(id, &color, &[8; 2], Intent::Relative, JpxSamples::Unsigned)
            .unwrap_err()
            .kind,
        ErrorKind::Syntax
    );
}

#[test]
fn lab_reader_features_do_not_bypass_selected_colour_interpretation() {
    use pagura_color::Intent;
    use pagura_core::ObjectId;
    use pagura_image::{ImageKind, ImageSource, jpx::JpxColor};
    for name in ["lab-fallback-illuminant", "lab-fallback-zero-range"] {
        let document = Document::open(
            pagura_testkit::fixture("jpx_lab_probe", &format!("{name}.pdf")),
            &OpenOptions::default(),
        )
        .unwrap();
        let id = ObjectId::new(5, 0);
        let source = ImageSource::from_xobject(&document, document.object(id).unwrap()).unwrap();
        let ImageKind::EmbeddedColor {
            color: color @ JpxColor::Alternatives(choices),
            ..
        } = source.kind()
        else {
            panic!("missing ranked Lab choices")
        };
        assert_eq!(choices.len(), 2);
        assert!(matches!(&choices[0].color, JpxColor::Lab(_)));
        let JpxColor::Lab(parameters) = &choices[1].color else {
            panic!("missing explicit D50 fallback")
        };
        assert_eq!(parameters.illuminant(), 0x0044_3530);
        assert_eq!(parameters.ranges(), [100, 170, 200]);
        let space = Colors::new(&document, None)
            .embedded_jpx(id, color, &[8; 3], Intent::Relative, JpxSamples::Unsigned)
            .unwrap();
        let ColorSpace::Calibrated(lab) = space.as_ref() else {
            panic!("missing selected D50 fallback")
        };
        assert_eq!(lab.lab_ranges().unwrap()[0], [0.0, 100.0]);
        // The selected default encoding makes (128,96) neutral, regardless
        // of the unsupported higher-ranked explicit colour record.
        let pixel = lab.convert(&[50.0, 0.0, 0.0]);
        assert!((pixel[0] - pixel[1]).abs() < 0.001);
        assert!((pixel[1] - pixel[2]).abs() < 0.001);
    }
}

#[test]
fn unsupported_lab_does_not_hide_an_explicit_rgb_alternative() {
    use pagura_color::Intent;
    use pagura_core::ObjectId;
    use pagura_image::{ImageKind, ImageSource};
    let document = Document::open(
        pagura_testkit::fixture("jpx_lab_probe", "lab-fallback-rgb.pdf"),
        &OpenOptions::default(),
    )
    .unwrap();
    let id = ObjectId::new(5, 0);
    let source = ImageSource::from_xobject(&document, document.object(id).unwrap()).unwrap();
    let ImageKind::EmbeddedColor { color, .. } = source.kind() else {
        panic!("missing colour choices")
    };
    let space = Colors::new(&document, None)
        .embedded_jpx(id, color, &[8; 3], Intent::Relative, JpxSamples::Unsigned)
        .unwrap();
    assert!(matches!(space.as_ref(), ColorSpace::DeviceRgb));
}
