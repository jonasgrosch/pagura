#![forbid(unsafe_code)]
#![expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "the vector helpers fail the test on unexpected fixture data"
)]
//! The independent image XObject, dictionary, inline-image and sample
//! vectors of `corpus/generated/image`, decoded through the public API. Each
//! fixture kind has its own test, and every failing case is listed rather
//! than only the first.
use pagura_core::{Dictionary, ErrorKind, Limits, ObjectId, Position, Recovery, Value};
use pagura_document::{Document, OpenOptions};
use pagura_image::{
    EnumeratedColorSpace, ImageKind, JpxColor, MaskKind, Samples, decode_inline, decode_xobject,
    inline_data_length,
};
use pagura_testkit::{fixture, fixture_names, fixture_path};
use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};

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

/// Runs `check` on every case of `kind` with its parameters and expected
/// output, then fails with the names of the cases that failed. Returns the
/// number of cases.
fn each_case(kind: &str, check: fn(&str, &BTreeMap<String, String>, &[u8])) -> usize {
    let (mut count, mut failures) = (0, Vec::new());
    for name in fixture_names("image", "params") {
        let params = options(&name);
        if params["kind"] != kind {
            continue;
        }
        count += 1;
        let expected = fixture("image", &format!("{name}.out"));
        if catch_unwind(AssertUnwindSafe(|| check(&name, &params, &expected))).is_err() {
            failures.push(name);
        }
    }
    assert_eq!(failures, Vec::<String>::new(), "failing {kind} vectors");
    count
}

#[test]
fn pdf_image_vectors() {
    let count = each_case("pdf-image", pdf_image_vector);
    assert!(count >= 100, "{count} image XObject vectors");
}

#[test]
fn dictionary_vectors() {
    let count = each_case("dictionary", dictionary_vector);
    assert!(count >= 10, "{count} dictionary vectors");
}

#[test]
fn inline_vectors() {
    let count = each_case("inline", inline_vector);
    assert!(count >= 30, "{count} inline vectors");
}

#[test]
fn sample_vectors() {
    let count = each_case("samples", sample_vector);
    assert!(count >= 30, "{count} sample vectors");
}

fn values(bytes: &[u8]) -> Vec<u16> {
    std::str::from_utf8(bytes)
        .unwrap()
        .split_whitespace()
        .map(|v| v.parse().unwrap())
        .collect()
}

fn pdf_image_vector(name: &str, params: &BTreeMap<String, String>, expected: &[u8]) {
    let document = Document::open(
        fixture("image", &format!("{name}.pdf")),
        &OpenOptions {
            recovery: if params["outcome"] == "repair" {
                Recovery::Repair
            } else {
                Recovery::Strict
            },
            ..OpenOptions::default()
        },
    )
    .unwrap();
    let result = decode_xobject(&document, document.object(ObjectId::new(3, 0)).unwrap());
    if params["outcome"] == "repair" {
        assert_repairs(name, params, &document);
    } else if params["outcome"] != "valid" {
        assert_rejected(name, params, &result.unwrap_err());
        return;
    }
    let image = result.unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(image.width(), params["width"].parse().unwrap(), "{name}");
    assert_eq!(image.height(), params["height"].parse().unwrap(), "{name}");
    assert_eq!(
        image.components(),
        params["components"].parse().unwrap(),
        "{name}"
    );
    // The fixtures write 0 for components of mixed precision.
    let bits: u8 = params["bits"].parse().unwrap();
    assert_eq!(
        image.bits_per_component(),
        (bits != 0).then_some(bits),
        "{name}"
    );
    assert_samples(image.samples(), &values(expected), name);
    if let Some(bits) = params.get("component_bits") {
        let bits: Vec<u8> = bits.split(',').map(|v| v.parse().unwrap()).collect();
        for (i, bits) in bits.into_iter().enumerate() {
            assert_eq!(image.component_bits(i), Some(bits), "{name}");
        }
    }
    assert_eq!(image.component_bits(usize::from(image.components())), None);
    if matches!(image.kind(), ImageKind::EmbeddedColor { .. }) {
        assert!(image.embedded_color_fallback().is_some());
    }
    if let Some(space) = params.get("space") {
        assert_eq!(space_name(image.kind()), space, "{name}");
    }
    if let Some(inverted) = params.get("inverted") {
        assert_eq!(
            image.kind(),
            &ImageKind::Stencil {
                inverted: inverted == "1"
            },
            "{name}"
        );
    }
    if let Some(decode) = params.get("decode") {
        let expected = if decode == "none" {
            None
        } else {
            Some(
                decode
                    .split(',')
                    .map(|v| v.parse().unwrap())
                    .collect::<Vec<f64>>(),
            )
        };
        assert_eq!(image.decode().map(<[f64]>::to_vec), expected, "{name}");
    }
    if params.get("mask").is_some_and(|s| s == "embedded") {
        let MaskKind::Embedded(alpha) = image.mask() else {
            panic!("{name}: missing decoded alpha");
        };
        assert_eq!(
            alpha.bits_per_component(),
            params["alpha_bits"].parse::<u8>().unwrap()
        );
        assert_eq!(
            alpha.premultiplied(),
            params.get("premultiplied").is_some_and(|v| v == "1")
        );
        assert_samples(
            alpha.samples(),
            &values(&fixture("image", &format!("{name}.alpha"))),
            name,
        );
    } else {
        assert_eq!(image.mask(), &MaskKind::None, "{name}");
    }
}

fn assert_rejected(name: &str, params: &BTreeMap<String, String>, error: &pagura_core::Error) {
    match params["outcome"].as_str() {
        "syntax" => assert_eq!(error.kind, ErrorKind::Syntax, "{name}: {error}"),
        "limit" => assert_eq!(error.kind, ErrorKind::Limit, "{name}: {error}"),
        _ => assert_eq!(
            error.feature().unwrap().as_str(),
            params["feature"],
            "{name}"
        ),
    }
    assert!(error.object.is_some(), "{name}");
    if params.get("reject_repair").is_some_and(|v| v == "1") {
        let repaired = Document::open(
            fixture("image", &format!("{name}.pdf")),
            &OpenOptions::default(),
        )
        .unwrap();
        let refused =
            decode_xobject(&repaired, repaired.object(ObjectId::new(3, 0)).unwrap()).unwrap_err();
        assert_eq!(refused.kind, error.kind, "{name}");
    }
}

fn space_name(kind: &ImageKind) -> &str {
    match kind {
        ImageKind::Color {
            color_space: Value::Name(name),
        } => std::str::from_utf8(name.as_bytes()).unwrap(),
        ImageKind::Color {
            color_space: Value::Array(array),
        } => std::str::from_utf8(array[0].as_name().unwrap().as_bytes()).unwrap(),
        ImageKind::EmbeddedColor {
            color: JpxColor::Icc(_),
        } => "icc",
        ImageKind::EmbeddedColor {
            color: JpxColor::Enumerated(EnumeratedColorSpace::SYCC),
        } => "sYCC",
        ImageKind::Stencil { .. } => "stencil",
        _ => panic!("unexpected image colour description"),
    }
}

fn assert_samples(samples: &Samples, expected: &[u16], name: &str) {
    assert_eq!(samples.len(), expected.len(), "{name}");
    for (i, &value) in expected.iter().enumerate() {
        assert_eq!(samples.get(i), Some(value), "{name} sample {i}");
    }
}

/// Repairs of the image dictionary are placed at the image object; repairs
/// of its data at an offset in the stream.
fn repair_placed(tag: &str, position: Position) -> bool {
    use pagura_image::repairs::{
        CCITT_COLUMNS, DECODE_PARAMS, FILTER_ABBREVIATION, STENCIL_COLOR_SPACE,
    };
    if [
        STENCIL_COLOR_SPACE,
        FILTER_ABBREVIATION,
        DECODE_PARAMS,
        CCITT_COLUMNS,
    ]
    .iter()
    .any(|repair| repair.as_str() == tag)
    {
        matches!(position, Position::File(_))
    } else {
        matches!(position, Position::Stream { .. })
    }
}

fn assert_repairs(name: &str, params: &BTreeMap<String, String>, document: &Document) {
    let strict = Document::open(
        fixture("image", &format!("{name}.pdf")),
        &OpenOptions {
            recovery: Recovery::Strict,
            ..OpenOptions::default()
        },
    )
    .unwrap();
    let error = decode_xobject(&strict, strict.object(ObjectId::new(3, 0)).unwrap()).unwrap_err();
    assert_eq!(error.repair.unwrap().as_str(), params["repair"], "{name}");
    assert_eq!(error.object, Some(ObjectId::new(3, 0)), "{name}");
    assert!(repair_placed(&params["repair"], error.position), "{name}");
    assert!(strict.diagnostics().is_empty(), "{name}");
    let reports = document.diagnostics();
    let expected_tags = params.get("report_tags").map_or_else(
        || vec![params["repair"].as_str(); params.get("reports").map_or(1, |n| n.parse().unwrap())],
        |tags| tags.split(',').collect(),
    );
    assert_eq!(
        reports
            .iter()
            .map(pagura_core::Diagnostic::tag)
            .collect::<Vec<_>>(),
        expected_tags,
        "{name}"
    );
    for report in &reports {
        assert_eq!(report.object, Some(ObjectId::new(3, 0)), "{name}");
        assert!(repair_placed(report.tag(), report.position), "{name}");
    }
    // A second use of the same XObject must not duplicate reports.
    decode_xobject(document, document.object(ObjectId::new(3, 0)).unwrap()).unwrap();
    assert_eq!(document.diagnostics(), reports, "{name}");
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
    assert_eq!(
        image.samples(),
        &Samples::Bytes(expected.to_vec()),
        "{name}"
    );
    if let Some(mask) = params.get("mask") {
        assert_eq!(
            image.mask(),
            &match mask.as_str() {
                "soft" => MaskKind::Soft(ObjectId::new(4, 0)),
                "stencil" => MaskKind::Stencil(ObjectId::new(4, 0)),
                "key" => MaskKind::ColorKey(vec![std::ops::RangeInclusive::new(12, 26)]),
                _ => panic!("invalid fixture"),
            }
        );
    }
    if params.contains_key("interpolate") {
        assert!(image.interpolate());
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
                inline_data_length(dict, &data, &Limits::default())
                    .unwrap_err()
                    .kind,
                error.kind,
                "{name}"
            );
        }
        return;
    }
    let image = result.unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(
        image.samples(),
        &Samples::Bytes(expected.to_vec()),
        "{name}"
    );
    if let Some(inverted) = params.get("stencil") {
        assert_eq!(
            image.kind(),
            &ImageKind::Stencil {
                inverted: inverted == "1"
            },
            "{name}"
        );
    }
    // Later inline images cannot be mistaken for ambiguous data markers.
    let content = [data.as_slice(), b" EI Q BI /L 2 ID xx EI Q"].concat();
    assert_eq!(
        inline_data_length(dict, &content, &Limits::default()).unwrap(),
        data.len(),
        "{name}"
    );
}

fn sample_vector(name: &str, p: &BTreeMap<String, String>, expected: &[u8]) {
    let document = Document::open(
        fixture("image", &format!("{name}.pdf")),
        &OpenOptions::default(),
    )
    .unwrap();
    let decoded = decode_xobject(&document, document.object(ObjectId::new(3, 0)).unwrap()).unwrap();
    assert_eq!(decoded.width(), 7);
    assert_eq!(decoded.height(), 3);
    assert_eq!(
        decoded.bits_per_component(),
        Some(u8::try_from(p["bits"].parse::<i64>().unwrap()).unwrap())
    );
    let bytes = match decoded.samples() {
        Samples::Bytes(v) => v.clone(),
        Samples::Words(v) => v.iter().flat_map(|v| v.to_be_bytes()).collect(),
    };
    assert_eq!(bytes, expected, "{name}");
}

#[test]
fn jpeg_reports_are_bounded_per_decode_and_do_not_authorize_strict_repairs() {
    let document = Document::open(
        fixture("image", "jpeg-padding-progressive.pdf"),
        &OpenOptions::default(),
    )
    .unwrap();
    let stream = document
        .image_stream(document.object(ObjectId::new(3, 0)).unwrap())
        .unwrap();
    let mut diagnostics = pagura_core::Diagnostics::new(Recovery::Repair);
    for _ in 0..2 {
        let image = pagura_imagecodec::dct::decode_with_diagnostics(
            &stream.data,
            None,
            &Limits::default(),
            &mut diagnostics,
        )
        .unwrap();
        assert_eq!(image.samples, [128]);
    }
    // Multiple repaired boundaries in each progressive JPEG produce one
    // record; a second decode still obtains its own permission and evidence.
    assert_eq!(diagnostics.as_slice().len(), 2);
    // Earlier evidence cannot enter a strict sink, so it can never authorize
    // a strict repair.
    let mut strict = pagura_core::Diagnostics::new(Recovery::Strict);
    assert!(strict.absorb(diagnostics.take()).is_err());
    assert!(strict.is_empty());
    let error = pagura_imagecodec::dct::decode_with_diagnostics(
        &stream.data,
        None,
        &Limits::default(),
        &mut strict,
    )
    .unwrap_err();
    assert_eq!(
        error.repair,
        Some(pagura_imagecodec::repairs::JPEG_ENTROPY_PADDING)
    );
}
