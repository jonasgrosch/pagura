use super::*;
use crate::features;
use pagura_core::{Dictionary, ErrorKind, Limits, ObjectId, Position, Recovery};
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
        assert_ne!(error.position, Position::Unknown, "{name}");
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
        let error = DefaultAppearance::parse(bytes, &Limits::default()).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax, "{error}");
    }
    assert_eq!(
        DefaultAppearance::parse(
            b"/F1 10 Tf",
            &Limits {
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
fn text_style_scales_the_declared_size_by_the_vertical_text_scale() {
    let style = default_text_style(b"0 g /Helv 10 Tf 1 0 0 2 0 0 Tm", &Limits::default()).unwrap();
    assert_eq!(style.font(), &Name::new(b"Helv"));
    assert_eq!(style.size(), 20.0);
    let error = default_text_style(b"/Helv 10 Tf 1 1 0 1 0 0 Tm", &Limits::default()).unwrap_err();
    assert_eq!(
        error.kind,
        ErrorKind::Unsupported(features::APPEARANCE_TEXT_MATRIX)
    );
}

#[test]
fn text_update_preserves_nested_marked_content_and_appends_when_absent() {
    let limits = Limits::default();
    let policy = UpdatePolicy::Specification;
    let original = b"q\n/Other BMC /Tx BMC q /Nested BMC (EMC) Tj EMC Q EMC EMC\nQ\n";
    let actual = update::replace(original, b"/Tx BMC (NEW) Tj EMC\n", &limits, policy).unwrap();
    assert_eq!(actual, b"q\n/Other BMC /Tx BMC (NEW) Tj EMC\n EMC\nQ\n");
    assert_eq!(
        update::replace(b"0 g", b"/Tx BMC EMC\n", &limits, policy).unwrap(),
        b"0 g\n/Tx BMC EMC\n"
    );
    assert!(update::replace(b"/Tx BMC", b"", &limits, policy).is_err());
    assert!(update::replace(b"/Tx BMC EMC /Tx BMC EMC", b"", &limits, policy).is_err());
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
}

#[test]
fn negative_font_size_is_valid_but_explicitly_unsupported() {
    let error = DefaultAppearance::parse(b"/F1 -10 Tf", &Limits::default()).unwrap_err();
    assert_eq!(
        error.kind,
        ErrorKind::Unsupported(features::APPEARANCE_TEXT_MATRIX)
    );
}

#[test]
fn text_updates_reject_sections_crossing_text_or_graphics_scopes() {
    for content in [
        b"BT /Tx BMC (OLD) Tj EMC ET".as_slice(),
        b"q /Tx BMC Q EMC",
        b"/Tx BMC q EMC Q",
    ] {
        let error = update::replace(
            content,
            b"/Tx BMC EMC",
            &Limits::default(),
            UpdatePolicy::Specification,
        )
        .unwrap_err();
        assert_eq!(
            error.kind,
            ErrorKind::Unsupported(features::APPEARANCE_UPDATE)
        );
    }
}

#[test]
fn compatibility_replaces_unmarked_text_without_touching_vector_artwork() {
    let limits = Limits::default();
    let policy = UpdatePolicy::PreserveExisting;
    let old = b"q 0.7 G 1 1 30 20 re S BT /F1 10 Tf [(old) 20 (value)] TJ ET Q";
    let text = b"/Tx BMC BT /F1 10 Tf (new) Tj ET EMC";
    let actual = update::replace(old, text, &limits, policy).unwrap();
    assert_eq!(
        actual,
        b"q\nq 0.7 G 1 1 30 20 re S BT /F1 10 Tf  ET Q\nQ\n/Tx BMC BT /F1 10 Tf (new) Tj ET EMC"
    );
    for content in [
        b"BT 4 Tr (clip) Tj ET".as_slice(),
        b"/Other Do",
        b"BT (unfinished) Tj",
    ] {
        assert!(update::replace(content, text, &limits, policy).is_err());
    }
}

#[test]
fn compatibility_standard_font_fallback_is_narrow_and_preserves_resources() {
    let document = Document::open(
        fixture("forms", "text-quadding-0.pdf"),
        &OpenOptions::default(),
    )
    .unwrap();
    let mut resources = Dictionary::new();
    assert!(supply_font(&document, &mut resources, b"CustomFont").is_err());
    assert!(resources.is_empty());
    supply_font(&document, &mut resources, b"Helvetica").unwrap();
    let before = resources.clone();
    supply_font(&document, &mut resources, b"Helvetica").unwrap();
    assert_eq!(resources, before);
    let fonts = resources.get(b"Font").unwrap().as_dictionary().unwrap();
    let font = fonts.get(b"Helvetica").unwrap().as_dictionary().unwrap();
    assert!(font.is_name(b"BaseFont", b"Helvetica"));
    assert!(font.is_name(b"Encoding", b"WinAnsiEncoding"));
}

#[test]
fn an_invalid_widget_index_is_a_caller_error_without_widget_context() {
    let document = Document::open(
        fixture("forms", "text-quadding-0.pdf"),
        &OpenOptions::default(),
    )
    .unwrap();
    let filler = document.form_filler().unwrap().unwrap();
    let error = prepare_widget(&filler, 99, None).unwrap_err();
    assert_eq!(error.kind, ErrorKind::InvalidArgument);
    assert_eq!(error.position, Position::Unknown);
}

#[test]
fn filled_widgets_regenerate_only_changed_or_requested_widgets() {
    let document =
        Document::open(fixture("forms", "render-text.pdf"), &OpenOptions::default()).unwrap();
    let mut filler = document.form_filler().unwrap().unwrap();
    let dictionary = filler.form().widgets[0].dictionary;
    let rect = filler.form().widgets[0].rectangle;
    let fonts = ProviderFonts::new(None, &document.limits());
    let never = |_: &Dictionary| -> Result<bool> { panic!("an untouched widget is not culled") };
    let widgets = FilledWidgets::new(&filler).unwrap();
    assert_eq!(
        widgets.appearance(dictionary, rect, &fonts, never).unwrap(),
        None
    );
    // Identity, not equal content, selects the widget.
    let copy = dictionary.clone();
    assert_eq!(
        widgets.appearance(&copy, rect, &fonts, never).unwrap(),
        None
    );
    filler.set(0, FillValue::Text("AB")).unwrap();
    let widgets = FilledWidgets::new(&filler).unwrap();
    let generated = widgets
        .appearance(dictionary, rect, &fonts, |_| Ok(false))
        .unwrap()
        .unwrap();
    assert_eq!(generated, prepare_widget(&filler, 0, None).unwrap());
    assert!(matches!(generated, Appearance::Generated(_)));
    assert_eq!(
        widgets.appearance(&copy, rect, &fonts, never).unwrap(),
        None
    );
}

#[test]
fn filled_widgets_keep_a_culled_stored_appearance_without_layout() {
    let document = Document::open(
        fixture("forms", "render-empty-bbox.pdf"),
        &OpenOptions::default(),
    )
    .unwrap();
    let mut filler = document.form_filler().unwrap().unwrap();
    filler.set(0, FillValue::Text("AB")).unwrap();
    let widget = &filler.form().widgets[0];
    let fonts = ProviderFonts::new(None, &document.limits());
    let widgets = FilledWidgets::new(&filler).unwrap();
    let mut seen = None;
    let kept = widgets
        .appearance(widget.dictionary, widget.rectangle, &fonts, |stream| {
            seen = Some(stream.clone());
            Ok(true)
        })
        .unwrap();
    assert_eq!(kept, Some(Appearance::Existing(ObjectId::new(80, 0))));
    assert!(seen.unwrap().contains_key(b"BBox"));
    let error = widgets
        .appearance(widget.dictionary, widget.rectangle, &fonts, |_| {
            Err(Error::syntax("form has invalid BBox"))
        })
        .unwrap_err();
    assert_eq!(error.message, "form has invalid BBox");
}

#[test]
fn filled_widgets_keep_stored_appearances_of_untouched_unsupported_fields() {
    for (kind, feature) in [
        ("type0", features::APPEARANCE_FONT_ENCODING),
        ("oblique-tm", features::APPEARANCE_TEXT_MATRIX),
    ] {
        for recovery in [Recovery::Strict, Recovery::Repair] {
            let name = format!("render-fallback-{kind}");
            let document = Document::open(
                fixture("forms", &format!("{name}.pdf")),
                &OpenOptions {
                    recovery,
                    ..Default::default()
                },
            )
            .unwrap();
            let mut filler = document.form_filler().unwrap().unwrap();
            assert!(filler.form().need_appearances, "{name}");
            let widget = &filler.form().widgets[0];
            let (dictionary, rect) = (widget.dictionary, widget.rectangle);
            let fonts = ProviderFonts::new(None, &document.limits());
            let kept = FilledWidgets::new(&filler)
                .unwrap()
                .appearance(dictionary, rect, &fonts, |_| Ok(false))
                .unwrap();
            assert!(matches!(kept, Some(Appearance::Existing(_))), "{name}");
            let diagnostics = document.diagnostics();
            assert_eq!(diagnostics.len(), 1, "{name}");
            assert_eq!(
                diagnostics[0].kind,
                pagura_core::DiagnosticKind::Information(crate::information::APPEARANCE_KEPT)
            );
            assert_eq!(diagnostics[0].object, Some(ObjectId::new(11, 0)));
            assert_ne!(diagnostics[0].position, Position::Unknown);
            filler.set(0, FillValue::Text("AB")).unwrap();
            let error = FilledWidgets::new(&filler)
                .unwrap()
                .appearance(dictionary, rect, &fonts, |_| Ok(false))
                .unwrap_err();
            assert_eq!(error.kind, ErrorKind::Unsupported(feature), "{name}");
            assert_eq!(document.diagnostics().len(), 1, "{name}");
        }
    }
}
