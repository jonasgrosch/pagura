use super::*;
use pagura_core::{ErrorKind, Recovery};
use pagura_document::{
    OpenOptions,
    forms::{ButtonValue, ChoiceSelection, FillValue},
};
use pagura_testkit::{fixture, fixture_path};
use std::collections::BTreeMap;

#[test]
fn independent_appearance_programs_and_existing_states() {
    let mut count = 0;
    for entry in std::fs::read_dir(fixture_path("forms", "")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "params") {
            continue;
        }
        let name = path.file_stem().unwrap().to_str().unwrap();
        let params = std::fs::read_to_string(&path).unwrap();
        let params: BTreeMap<_, _> = params.lines().map(|l| l.split_once('=').unwrap()).collect();
        if params["appearance"].is_empty() {
            continue;
        }
        count += 1;
        check_case(name, &params);
    }
    assert!(count >= 20);
}
fn check_case(name: &str, params: &BTreeMap<&str, &str>) {
    let document = Document::open(
        fixture("forms", &format!("{name}.pdf")),
        &OpenOptions {
            recovery: Recovery::Strict,
            ..Default::default()
        },
    )
    .unwrap();
    let mut filler = document.form_filler().unwrap().unwrap();
    let value = String::from_utf8(fixture("forms", &format!("{name}.value"))).unwrap();
    let indices: Vec<usize> = if params["mode"] == "indices" {
        value
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|n| n.parse().unwrap())
            .collect()
    } else {
        Vec::new()
    };
    let state = Name::new(value.as_bytes());
    if params["changed"] == "true" {
        let input = match params["mode"] {
            "text" => FillValue::Text(&value),
            "indices" => FillValue::Choice(ChoiceSelection::Indices(&indices)),
            "choice-text" => FillValue::Choice(ChoiceSelection::Text(&value)),
            "widget" => FillValue::Button(ButtonValue::Widget {
                index: value.parse().unwrap(),
                state: None,
            }),
            "off" => FillValue::Button(ButtonValue::Off),
            "state" => FillValue::Button(ButtonValue::State(&state)),
            "button-export" => FillValue::Button(ButtonValue::Export(&value)),
            _ => panic!("unknown fixture input"),
        };
        filler.set(0, input).unwrap();
    }
    if let Some(expected) = params["appearance"].strip_prefix("error:") {
        let error = prepare(&filler, None).unwrap_err();
        let actual = error.feature().map_or_else(
            || error.kind.name().to_string(),
            |f| format!("unsupported:{f}"),
        );
        assert_eq!(actual, expected, "{name}: {error}");
        assert!(error.object.is_some(), "{name}");
        assert_ne!(error.position, pagura_core::Position::Unknown, "{name}");
        return;
    }
    let appearances = prepare(&filler, None).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(appearances[0].widget(), 0);
    match appearances[0].appearance() {
        Appearance::Generated(stream) => {
            assert_eq!(params["appearance"], "generated", "{name}");
            assert_eq!(
                String::from_utf8_lossy(stream.content()),
                String::from_utf8_lossy(&fixture("forms", &format!("{name}.content"))),
                "{name}"
            );
            assert!(stream.dictionary().is_name(b"Subtype", b"Form"));
            assert!(stream.dictionary().contains_key(b"Resources"));
            assert!(!stream.dictionary().contains_key(b"Filter"));
            assert!(!stream.dictionary().contains_key(b"Length"));
        }
        Appearance::Existing(id) => assert_eq!(
            params["appearance"],
            format!("existing:{}", id.number),
            "{name}"
        ),
        Appearance::Hidden => assert_eq!(params["appearance"], "hidden", "{name}"),
    }
    assert!(document.diagnostics().is_empty(), "{name}");
}
#[test]
fn default_appearance_errors_are_positioned_and_bounded() {
    for bytes in [
        b"/F1 10 Tf 1 0 Tm".as_slice(),
        b"/F1 10 Tf 1 0 0 1 0 0 Tm 1 0 0 1 0 0 Tm",
        b"0 g",
        b"/F1 10 Tf 2 2 rg",
    ] {
        let error = DefaultAppearance::parse(bytes, Limits::default()).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax, "{error}");
    }
    assert_eq!(
        DefaultAppearance::parse(
            b"/F1 10 Tf",
            Limits {
                operations: 1,
                ..Default::default()
            }
        )
        .unwrap_err()
        .kind,
        ErrorKind::Limit
    );
}
#[test]
fn text_update_preserves_nested_marked_content_and_appends_when_absent() {
    let original = b"q\n/Other BMC /Tx BMC q /Nested BMC (EMC) Tj EMC Q EMC EMC\nQ\n";
    let actual = update::replace(original, b"/Tx BMC (NEW) Tj EMC\n", Limits::default()).unwrap();
    assert_eq!(actual, b"q\n/Other BMC /Tx BMC (NEW) Tj EMC\n EMC\nQ\n");
    assert_eq!(
        update::replace(b"0 g", b"/Tx BMC EMC\n", Limits::default()).unwrap(),
        b"0 g\n/Tx BMC EMC\n"
    );
    assert!(update::replace(b"/Tx BMC", b"", Limits::default()).is_err());
    assert!(update::replace(b"/Tx BMC EMC /Tx BMC EMC", b"", Limits::default()).is_err());
}
#[test]
fn prepared_resources_keep_existing_font_collisions() {
    let document = Document::open(
        fixture("forms", "text-quadding-0.pdf"),
        &OpenOptions::default(),
    )
    .unwrap();
    let mut defaults = Dictionary::new();
    let mut old = Dictionary::new();
    let mut fonts = Dictionary::new();
    fonts.insert(Name::new(b"F1"), Value::Reference(ObjectId::new(70, 0)));
    defaults.insert(Name::new(b"Font"), Value::Dictionary(fonts.clone()));
    fonts.insert(Name::new(b"F1"), Value::Reference(ObjectId::new(71, 0)));
    old.insert(Name::new(b"Font"), Value::Dictionary(fonts));
    let merged = update::resources(&document, Some(&old), Some(&defaults)).unwrap();
    assert_eq!(
        merged
            .get(b"Font")
            .unwrap()
            .as_dictionary()
            .unwrap()
            .get(b"F1")
            .unwrap()
            .as_reference(),
        Some(ObjectId::new(71, 0))
    );
}

#[test]
fn resource_arrays_and_null_entries_keep_dictionary_semantics() {
    let document = Document::open(
        fixture("forms", "text-quadding-0.pdf"),
        &OpenOptions::default(),
    )
    .unwrap();
    let mut defaults = Dictionary::new();
    defaults.insert(
        Name::new(b"ProcSet"),
        Value::Array(vec![Value::name(b"Text")]),
    );
    let mut fonts = Dictionary::new();
    fonts.insert(Name::new(b"F1"), Value::Reference(ObjectId::new(70, 0)));
    defaults.insert(Name::new(b"Font"), Value::Dictionary(fonts));
    let mut existing = Dictionary::new();
    existing.insert(
        Name::new(b"ProcSet"),
        Value::Array(vec![Value::name(b"PDF")]),
    );
    existing.insert(Name::new(b"Font"), Value::Null);
    let result = update::resources(&document, Some(&existing), Some(&defaults)).unwrap();
    assert_eq!(result.get(b"ProcSet"), existing.get(b"ProcSet"));
    assert_eq!(result.get(b"Font"), defaults.get(b"Font"));
}

#[test]
fn appearance_storage_and_output_limits_fail_without_partial_results() {
    let document = Document::open(
        fixture("forms", "text-quadding-0.pdf"),
        &OpenOptions {
            limits: Limits {
                document_bytes: 64_000,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    let mut filler = document.form_filler().unwrap().unwrap();
    filler.set(0, FillValue::Text("AB")).unwrap();
    assert!(prepare(&filler, None).is_ok());
    filler.set(0, FillValue::Text(&"AB".repeat(5_000))).unwrap();
    let error = prepare(&filler, None).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
    assert!(error.object.is_some());
    let mut out = Program::new(Limits {
        stream_bytes: 10,
        ..Default::default()
    });
    out.text("1234567890").unwrap();
    assert_eq!(out.text("x").unwrap_err().kind, ErrorKind::Limit);
    assert_eq!(out.finish(), b"1234567890");
}

#[test]
fn negative_font_size_is_valid_but_explicitly_unsupported() {
    let error = DefaultAppearance::parse(b"/F1 -10 Tf", Limits::default()).unwrap_err();
    assert_eq!(
        error.kind,
        ErrorKind::Unsupported(crate::features::APPEARANCE_TEXT_MATRIX)
    );
}

#[test]
fn text_updates_reject_sections_crossing_text_or_graphics_scopes() {
    for content in [
        b"BT /Tx BMC (OLD) Tj EMC ET".as_slice(),
        b"q /Tx BMC Q EMC",
        b"/Tx BMC q EMC Q",
    ] {
        let error = update::replace(content, b"/Tx BMC EMC", Limits::default()).unwrap_err();
        assert_eq!(
            error.kind,
            ErrorKind::Unsupported(crate::features::APPEARANCE_UPDATE)
        );
    }
}
