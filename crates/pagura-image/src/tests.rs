//! Dictionary invariants, inline abbreviations and fax parameters.
use crate::{ImageKind, MaskKind, Samples, decode_inline, decode_xobject, inline_data_length};
use pagura_core::{Dictionary, ErrorKind, Limits, Name, ObjectId, Recovery, Value};
use pagura_document::{Document, OpenOptions};
use pagura_imagecodec::ccitt;
use pagura_testkit::fixture;

fn dictionary(entries: &[(&[u8], Value)]) -> Dictionary {
    entries
        .iter()
        .map(|(key, value)| (Name::new(key), value.clone()))
        .collect()
}
fn document() -> Document {
    Document::open(
        fixture("image", "samples-1-1-raw.pdf"),
        &OpenOptions::default(),
    )
    .unwrap()
}

#[test]
fn decode_arrays_stencils_color_keys_and_inline_abbreviations() {
    let document = document();
    let mut dict = dictionary(&[
        (b"W", Value::Integer(3)),
        (b"H", Value::Integer(2)),
        (b"BPC", Value::Integer(2)),
        (b"CS", Value::name(b"G")),
        (b"I", Value::Boolean(true)),
        (
            b"D",
            Value::Array(vec![Value::Integer(1), Value::Integer(0)]),
        ),
    ]);
    let image = decode_inline(
        &document,
        &Dictionary::new(),
        &dict,
        &[0b0001_1000, 0b1110_0100],
    )
    .unwrap();
    assert_eq!(image.samples(), &Samples::Bytes(vec![0, 1, 2, 3, 2, 1]));
    assert_eq!(image.decode(), Some([1., 0.].as_slice()));
    assert!(image.interpolate());
    assert_eq!(
        inline_data_length(&dict, b"12 EI Q", &Limits::default()).unwrap(),
        2
    );
    dict.insert(Name::new(b"L"), Value::Integer(5));
    assert_eq!(
        inline_data_length(&dict, b"x EI  EI Q", &Limits::default()).unwrap(),
        5
    );
    dict.remove(b"L");
    dict.insert(Name::new(b"F"), Value::name(b"AHx"));
    assert_eq!(
        inline_data_length(&dict, b"1864> EI Q", &Limits::default()).unwrap(),
        5
    );
    assert!(inline_data_length(&dict, b"x EI y EI Q", &Limits::default()).is_err());
    let stencil = dictionary(&[
        (b"W", Value::Integer(2)),
        (b"H", Value::Integer(1)),
        (b"IM", Value::Boolean(true)),
        (
            b"D",
            Value::Array(vec![Value::Integer(1), Value::Integer(0)]),
        ),
    ]);
    let image = decode_inline(&document, &Dictionary::new(), &stencil, &[0x80]).unwrap();
    assert_eq!(image.kind(), &ImageKind::Stencil { inverted: true });
    let mut keyed = dictionary(&[
        (b"Width", Value::Integer(2)),
        (b"Height", Value::Integer(1)),
        (b"BitsPerComponent", Value::Integer(8)),
        (b"ColorSpace", Value::name(b"DeviceGray")),
        (
            b"Mask",
            Value::Array(vec![Value::Integer(12), Value::Integer(25)]),
        ),
    ]);
    let image = crate::dispatch::decode(
        &document,
        &keyed,
        &[12, 26],
        None,
        &mut pagura_core::Diagnostics::new(Recovery::Strict),
    )
    .unwrap();
    assert_eq!(
        image.mask(),
        &MaskKind::ColorKey(vec![std::ops::RangeInclusive::new(12, 25)])
    );
    keyed.insert(Name::new(b"Decode"), Value::Array(vec![Value::Integer(0)]));
    assert_eq!(
        crate::dispatch::decode(
            &document,
            &keyed,
            &[12, 26],
            None,
            &mut pagura_core::Diagnostics::new(Recovery::Strict)
        )
        .unwrap_err()
        .kind,
        ErrorKind::Syntax
    );
}

#[test]
fn image_budgets_precede_allocations() {
    // The codecs' own checks are in pagura-imagecodec's image_vectors tests.
    let limits = Limits {
        pixels: 1,
        decoded_bytes: 1024,
        ..Limits::default()
    };
    let doc = Document::open(
        fixture("image", "samples-1-1-raw.pdf"),
        &OpenOptions {
            limits,
            recovery: Recovery::Strict,
            ..OpenOptions::default()
        },
    )
    .unwrap();
    assert_eq!(
        decode_xobject(&doc, doc.object(ObjectId::new(3, 0)).unwrap())
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
}

#[test]
fn fax_parameters_defaults_and_type_validation() {
    assert_eq!(
        crate::adapter::fax::params(None).unwrap(),
        ccitt::CcittParams::default()
    );
    for (key, value) in [
        (b"Columns".as_slice(), Value::Integer(0)),
        (b"Rows", Value::Integer(-1)),
        (b"BlackIs1", Value::Integer(1)),
        (b"K", Value::Boolean(false)),
        (b"DamagedRowsBeforeError", Value::Integer(-1)),
    ] {
        assert_eq!(
            crate::adapter::fax::params(Some(&dictionary(&[(key, value)])))
                .unwrap_err()
                .kind,
            ErrorKind::Syntax
        );
    }
}
