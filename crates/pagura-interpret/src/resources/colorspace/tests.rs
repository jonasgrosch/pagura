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
        .embedded_jpx(id, &color, 3, intent, JpxSamples::Unsigned)
        .unwrap();
    let other = Colors::new(&document, Some(&store))
        .embedded_jpx(id, &color, 3, intent, JpxSamples::Unsigned)
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
            .embedded_jpx(id, &color, 1, intent, JpxSamples::Unsigned)
            .unwrap_err()
            .kind,
        ErrorKind::Syntax
    ));
    assert!(matches!(
        colors
            .embedded_jpx(
                ObjectId::new(5, 0),
                &pagura_image::jpx::JpxColor::Icc(b"broken".to_vec()),
                3,
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
            .embedded_jpx(id, &color, 3, intent, JpxSamples::Unsigned)
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
            .embedded_jpx(id, color, 3, intent, JpxSamples::Unsigned)
            .unwrap();
        assert_eq!(
            matches!(space.as_ref(), ColorSpace::Icc(_)),
            intent == Intent::Perceptual
        );
    }
}
