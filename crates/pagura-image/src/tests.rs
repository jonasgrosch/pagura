//! Independent generated image vectors and dictionary invariants.
use crate::{
    DecodeRequest, DecodedImage, ImageId, ImageKind, ImageSource, MaskKind, Samples, ccitt, dct,
    inline_data_length,
};
use pagura_core::{Dictionary, ErrorKind, Limits, Name, ObjectId, Recovery, Value};
use pagura_document::{Document, IndirectObject, OpenOptions};
use pagura_testkit::{fixture, fixture_path};
use std::collections::BTreeMap;

/// Decodes an image XObject through its source as painting does, every
/// sample exactly, and reports the decode's repairs to `document`.
pub(crate) fn decode_xobject(
    document: &Document,
    image: &IndirectObject,
) -> pagura_document::source::ReadResult<DecodedImage> {
    let source = ImageSource::from_xobject(document, image)?;
    let decoded = source.decode(
        &DecodeRequest::FULL,
        &document.limits(),
        document.recovery(),
    )?;
    document.report(decoded.diagnostics);
    Ok(decoded.image)
}

/// [`decode_xobject`] for an inline image with `resources`.
pub(crate) fn decode_inline(
    document: &Document,
    resources: &Dictionary,
    dictionary: &Dictionary,
    data: &[u8],
) -> pagura_document::source::ReadResult<DecodedImage> {
    let id = ImageId::Inline {
        stream: None,
        offset: 0,
    };
    let source = ImageSource::from_inline(document, resources, dictionary, data, id)?;
    let decoded = source.decode(
        &DecodeRequest::FULL,
        &document.limits(),
        document.recovery(),
    )?;
    document.report(decoded.diagnostics);
    Ok(decoded.image)
}

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
fn options(name: &str) -> BTreeMap<String, String> {
    String::from_utf8(fixture("image", &format!("{name}.params")))
        .unwrap()
        .lines()
        .map(|line| {
            let (k, v) = line.split_once('=').unwrap();
            (k.to_owned(), v.to_owned())
        })
        .collect()
}

#[test]
fn independent_vectors() {
    let names: Vec<_> = std::fs::read_dir(fixture_path("image", ""))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "params"))
        .map(|p| p.file_stem().unwrap().to_str().unwrap().to_owned())
        .collect();
    assert!(names.len() >= 60);
    for name in names {
        let p = options(&name);
        let number =
            |key: &str, default: i64| p.get(key).map_or(default, |v| v.parse::<i64>().unwrap());
        let expected = fixture("image", &format!("{name}.out"));
        if p["kind"] == "pdf-image" {
            crate::pdf_tests::vector(&name, &p, &expected);
            continue;
        }
        if p["kind"] == "dictionary" {
            dictionary_vector(&name, &p, &expected);
            continue;
        }
        if p["kind"] == "inline" {
            inline_vector(&name, &p, &expected);
            continue;
        }
        if p["kind"] == "samples" {
            sample_vector(&name, &p, &expected);
            continue;
        }
        let encoded = fixture("image", &format!("{name}.in"));
        let limits = Limits::default();
        let result = if p["kind"] == "dct" {
            dct::decode(
                &encoded,
                match number("transform", -1) {
                    -1 => None,
                    v => Some(v),
                },
                &limits,
            )
            .map(|image| image.samples)
        } else {
            let params = ccitt::CcittParams {
                k: number("k", 0),
                columns: u32::try_from(number("width", 13)).unwrap(),
                rows: u32::try_from(number("height", 6)).ok().filter(|&v| v > 0),
                end_of_block: number("eob", 1) != 0,
                end_of_line: number("eol", 0) != 0,
                encoded_byte_align: number("align", 0) != 0,
                black_is_1: number("black", 0) != 0,
                damaged_rows_before_error: u32::try_from(number("damaged", 0)).unwrap(),
            };
            if p["outcome"] == "repair" {
                let error = ccitt::decode(&encoded, &params, &limits).unwrap_err();
                assert_eq!(error.repair, Some(ccitt::tags::DAMAGED_ROW));
                let mut diagnostics = pagura_core::Diagnostics::new(Recovery::Repair);
                let decoded =
                    ccitt::decode_with_diagnostics(&encoded, &params, &limits, &mut diagnostics)
                        .unwrap();
                assert_eq!(decoded, expected, "{name}");
                assert_eq!(
                    diagnostics.as_slice().len(),
                    usize::try_from(params.damaged_rows_before_error).unwrap()
                );
                continue;
            }
            ccitt::decode(&encoded, &params, &limits)
        };
        match p["outcome"].as_str() {
            "valid" => {
                let actual = result.unwrap_or_else(|e| panic!("{name}: {e}"));
                assert_eq!(actual.len(), expected.len(), "{name}");
                let tolerance = u8::try_from(number("tolerance", 0)).unwrap();
                for (i, (&a, &b)) in actual.iter().zip(&expected).enumerate() {
                    assert!(a.abs_diff(b) <= tolerance, "{name} byte {i}: {a} != {b}");
                }
            }
            "unsupported" => {
                assert_eq!(
                    result.unwrap_err().feature().unwrap().as_str(),
                    p["feature"],
                    "{name}"
                );
            }
            outcome => {
                let error = result.unwrap_err();
                assert_eq!(
                    error.kind,
                    if outcome == "limit" {
                        ErrorKind::Limit
                    } else {
                        ErrorKind::Syntax
                    },
                    "{name}: {error}"
                );
            }
        }
    }
}

#[test]
fn decode_arrays_stencils_and_inline_abbreviations() {
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
    assert_eq!(image.samples, Samples::Bytes(vec![0, 1, 2, 3, 2, 1]));
    assert_eq!(image.decode, Some(vec![1., 0.]));
    assert!(image.interpolate);
    assert_eq!(inline_data_length(&dict, b"12 EI Q").unwrap(), 2);
    dict.insert(Name::new(b"L"), Value::Integer(5));
    assert_eq!(inline_data_length(&dict, b"x EI  EI Q").unwrap(), 5);
    dict.remove(b"L");
    dict.insert(Name::new(b"F"), Value::name(b"AHx"));
    assert_eq!(inline_data_length(&dict, b"1864> EI Q").unwrap(), 5);
    assert!(inline_data_length(&dict, b"x EI y EI Q").is_err());
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
    assert_eq!(image.kind, ImageKind::Stencil { inverted: true });
}

#[test]
fn image_and_codec_budgets_precede_allocations() {
    let limits = Limits {
        pixels: 1,
        decoded_bytes: 1024,
        ..Limits::default()
    };
    assert_eq!(
        dct::decode(&fixture("image", "jpeg-baseline-gray.in"), None, &limits)
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
    let params = ccitt::CcittParams {
        columns: 13,
        rows: Some(6),
        ..ccitt::CcittParams::default()
    };
    assert_eq!(
        ccitt::decode(&[], &params, &limits).unwrap_err().kind,
        ErrorKind::Limit
    );
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
    let mut params = params;
    params.columns = 0;
    assert_eq!(
        ccitt::decode(&[], &params, &Limits::default())
            .unwrap_err()
            .kind,
        ErrorKind::Syntax
    );
}

#[test]
fn fax_parameters_defaults_and_type_validation() {
    assert_eq!(
        ccitt::CcittParams::from_dictionary(None).unwrap(),
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
            ccitt::CcittParams::from_dictionary(Some(&dictionary(&[(key, value)])))
                .unwrap_err()
                .kind,
            ErrorKind::Syntax
        );
    }
}

fn dictionary_vector(name: &str, params: &BTreeMap<String, String>, expected: &[u8]) {
    let doc = Document::open(
        fixture("image", &format!("{name}.pdf")),
        &OpenOptions::default(),
    )
    .unwrap();
    let result = decode_xobject(&doc, doc.object(ObjectId::new(3, 0)).unwrap());
    if params["outcome"] != "valid" {
        let error = result.unwrap_err();
        assert_eq!(
            error.kind,
            if params["outcome"] == "limit" {
                ErrorKind::Limit
            } else {
                ErrorKind::Syntax
            },
            "{name}"
        );
        assert!(error.object.is_some());
        return;
    }
    let image = result.unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(image.samples, Samples::Bytes(expected.to_vec()), "{name}");
    if let Some(mask) = params.get("mask") {
        assert_eq!(
            image.mask,
            match mask.as_str() {
                "soft" => MaskKind::Soft(ObjectId::new(4, 0)),
                "stencil" => MaskKind::Stencil(ObjectId::new(4, 0)),
                "key" => MaskKind::ColorKey(vec![(12, 26)]),
                _ => panic!("invalid fixture"),
            }
        );
    }
    if params.contains_key("interpolate") {
        assert!(image.interpolate);
    }
}

fn inline_vector(name: &str, params: &BTreeMap<String, String>, expected: &[u8]) {
    use pagura_core::syntax::{Origin, Parser};
    let bytes = fixture("image", &format!("{name}.dict"));
    let dict = Parser::new(&bytes, Origin::Buffer, Limits::default())
        .parse()
        .unwrap();
    let dict = dict.as_dictionary().unwrap();
    let data = fixture("image", &format!("{name}.in"));
    let resources = if fixture_path("image", &format!("{name}.resources")).exists() {
        let bytes = fixture("image", &format!("{name}.resources"));
        Parser::new(&bytes, Origin::Buffer, Limits::default())
            .parse()
            .unwrap()
            .as_dictionary()
            .unwrap()
            .clone()
    } else {
        Dictionary::new()
    };
    let doc = Document::open(
        fixture("image", "samples-1-1-raw.pdf"),
        &OpenOptions {
            recovery: if params["outcome"] == "valid" {
                Recovery::Strict
            } else {
                Recovery::Repair
            },
            ..OpenOptions::default()
        },
    )
    .unwrap();
    let result = decode_inline(&doc, &resources, dict, &data);
    if params["outcome"] == "repair" {
        let strict = Document::open(
            fixture("image", "samples-1-1-raw.pdf"),
            &OpenOptions {
                recovery: Recovery::Strict,
                ..OpenOptions::default()
            },
        )
        .unwrap();
        let error = decode_inline(&strict, &resources, dict, &data).unwrap_err();
        assert_eq!(error.repair.unwrap().as_str(), params["repair"], "{name}");
        assert_eq!(doc.diagnostics().len(), 1, "{name}");
        assert_eq!(doc.diagnostics()[0].tag(), params["repair"], "{name}");
    } else if params["outcome"] != "valid" {
        let error = result.unwrap_err();
        if params["outcome"] == "unsupported" {
            assert_eq!(
                error.feature().unwrap().as_str(),
                params["feature"],
                "{name}"
            );
        } else {
            assert_eq!(error.kind, ErrorKind::Syntax, "{name}");
        }
        if params.contains_key("boundary_error") {
            assert_eq!(
                inline_data_length(dict, &data).unwrap_err().kind,
                error.kind,
                "{name}"
            );
        }
        return;
    }
    let image = result.unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(image.samples, Samples::Bytes(expected.to_vec()), "{name}");
    if let Some(inverted) = params.get("stencil") {
        assert_eq!(
            image.kind,
            ImageKind::Stencil {
                inverted: inverted == "1"
            },
            "{name}"
        );
    }
    // Later inline images cannot be mistaken for ambiguous data markers.
    let content = [data.as_slice(), b" EI Q BI /L 2 ID xx EI Q"].concat();
    assert_eq!(
        inline_data_length(dict, &content).unwrap(),
        data.len(),
        "{name}"
    );
}

#[test]
fn truncated_and_mutated_codecs_never_invent_samples() {
    let limits = Limits {
        pixels: 4096,
        decoded_bytes: 65536,
        operations: 100_000,
        ..Limits::default()
    };
    for name in [
        "jpeg-baseline-gray",
        "jpeg-progressive-band-refinement",
        "jpeg-interleaved-progressive",
    ] {
        let data = fixture("image", &format!("{name}.in"));
        for end in 0..data.len() {
            assert!(
                dct::decode(&data[..end], None, &limits).is_err(),
                "{name} truncation {end}"
            );
        }
        for index in 0..data.len() {
            let mut corrupt = data.clone();
            corrupt[index] ^= 0xa5;
            let _ = dct::decode(&corrupt, None, &limits);
        }
    }
    let data = fixture("image", "fax-complex--1.in");
    let params = ccitt::CcittParams {
        k: -1,
        columns: 73,
        rows: None,
        ..ccitt::CcittParams::default()
    };
    for end in 0..data.len() {
        assert!(
            ccitt::decode(&data[..end], &params, &limits).is_err(),
            "fax truncation {end}"
        );
    }
}

fn sample_vector(name: &str, p: &BTreeMap<String, String>, expected: &[u8]) {
    let document = Document::open(
        fixture("image", &format!("{name}.pdf")),
        &OpenOptions::default(),
    )
    .unwrap();
    let decoded = decode_xobject(&document, document.object(ObjectId::new(3, 0)).unwrap()).unwrap();
    assert_eq!(decoded.width, 7);
    assert_eq!(decoded.height, 3);
    assert_eq!(
        decoded.bits_per_component,
        u8::try_from(p["bits"].parse::<i64>().unwrap()).unwrap()
    );
    let bytes = match decoded.samples {
        Samples::Planar(_) | Samples::Packed(_) => unreachable!("interleaved layout"),
        Samples::Bytes(v) => v,
        Samples::Words(v) | Samples::Mixed { values: v, .. } => {
            v.iter().flat_map(|v| v.to_be_bytes()).collect()
        }
    };
    assert_eq!(bytes, expected, "{name}");
}

#[test]
fn deferred_jpeg_height_is_an_explicit_capability_gap() {
    let mut encoded = fixture("image", "jpeg-baseline-gray.in");
    let frame = encoded.windows(2).position(|w| w == [255, 192]).unwrap();
    encoded[frame + 5..frame + 7].fill(0);
    let error = dct::decode(&encoded, None, &Limits::default()).unwrap_err();
    assert_eq!(error.feature(), Some(crate::features::JPEG_PROCESS));
}
