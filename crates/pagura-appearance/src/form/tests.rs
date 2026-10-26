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
fn shared_border_budgets_protect_filled_appearances() {
    for (name, segments) in [("Tx-rounded-72.pdf", 23), ("Tx-B-72.pdf", 15)] {
        let document = Document::open(
            fixture("form_borders", name),
            &OpenOptions {
                limits: Limits {
                    path_segments: segments,
                    ..Limits::default()
                },
                ..OpenOptions::default()
            },
        )
        .unwrap();
        let mut filler = document.form_filler().unwrap().unwrap();
        filler.set(0, FillValue::Text("AB")).unwrap();
        assert_eq!(
            prepare(&filler, None).unwrap_err().kind,
            ErrorKind::Limit,
            "{name}"
        );
    }
}

#[test]
fn style_reader_rejects_malformed_borders_and_rotations() {
    let document = document_for_style();
    for source in [
        b"<< /BS << /S 42 >> >>".as_slice(),
        b"<< /BS << /S /D /D [-1 2] >> >>",
        b"<< /BS << /S /D /D [0 0] >> >>",
        b"<< /Border [-1 2 1] >>",
        b"<< /MK << /R 45 >> >>",
        b"<< /MK << /R 90.0 >> >>",
        b"<< /MK << /R (90) >> >>",
        b"<< /MK << /R true >> >>",
    ] {
        let parsed = pagura_core::syntax::Parser::new(
            source,
            pagura_core::syntax::Origin::Buffer,
            Limits::default(),
        )
        .parse()
        .unwrap();
        assert!(matches!(
            Style::read(&document, parsed.as_dictionary().unwrap()),
            Err(Error {
                kind: ErrorKind::Syntax,
                ..
            })
        ));
    }
}

fn document_for_style() -> Document {
    Document::open(
        fixture("forms", "text-quadding-0.pdf"),
        &OpenOptions::default(),
    )
    .unwrap()
}

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
    let document = document_for_style();
    let resources = Dictionary::new();
    let policy = UpdatePolicy::Specification;
    let original = b"q\n/Other BMC /Tx BMC q /Nested BMC (EMC) Tj EMC Q EMC EMC\nQ\n";
    let actual = update::replace(
        &document,
        &resources,
        original,
        b"/Tx BMC (NEW) Tj EMC\n",
        policy,
    )
    .unwrap();
    assert_eq!(actual, b"q\n/Other BMC /Tx BMC (NEW) Tj EMC\n EMC\nQ\n");
    assert_eq!(
        update::replace(&document, &resources, b"0 g", b"/Tx BMC EMC\n", policy).unwrap(),
        b"0 g\n/Tx BMC EMC\n"
    );
    assert!(update::replace(&document, &resources, b"/Tx BMC", b"", policy).is_err());
    assert_eq!(
        update::replace(
            &document,
            &resources,
            b"/Tx BMC EMC /Tx BMC EMC",
            b"",
            policy
        )
        .unwrap(),
        b" "
    );
}

#[test]
fn updates_frame_binary_inline_samples_before_finding_repeated_tx_sections() {
    let document = document_for_style();
    let mut resources = Dictionary::new();
    let mut spaces = Dictionary::new();
    spaces.insert(Name::new(b"Samples"), Value::name(b"DeviceGray"));
    resources.insert(Name::new(b"ColorSpace"), Value::Dictionary(spaces));
    // The five samples themselves spell an apparent EI delimiter. Framing
    // uses the declared row size, not a substring search through samples.
    let artwork = b"q BI /W 5 /H 1 /BPC 8 /CS /Samples ID x EI  EI Q\n";
    let old = [
        artwork.as_slice(),
        b"/Other BMC /Tx << /MCID 1 >> BDC q /Tx BMC (old) Tj EMC Q EMC EMC\n",
        b"BX /Ignored 17 Future EX /Tx BMC (old) Tj EMC % retained\n",
    ]
    .concat();
    let text = b"/Tx BMC (new) Tj EMC";
    let result = update::replace(
        &document,
        &resources,
        &old,
        text,
        UpdatePolicy::Specification,
    )
    .unwrap();
    assert_eq!(
        result,
        [
            artwork.as_slice(),
            b"/Other BMC /Tx BMC (new) Tj EMC EMC\n",
            b"BX /Ignored 17 Future EX /Tx BMC (new) Tj EMC % retained\n",
        ]
        .concat()
    );
    let unmarked = [artwork.as_slice(), b"BT (old) Tj ET"].concat();
    let result = update::replace(
        &document,
        &resources,
        &unmarked,
        text,
        UpdatePolicy::PreserveExisting,
    )
    .unwrap();
    assert_eq!(
        result,
        [b"q\n".as_slice(), artwork, b"BT  ET\nQ\n", text].concat()
    );
}

#[test]
fn update_syntax_and_budgets_cover_the_shared_reader() {
    let document = document_for_style();
    let resources = Dictionary::new();
    for old in [
        b"BI /W 4 /H 1 /BPC 8 /CS /G ID 1234 EN".as_slice(),
        b"/Tx BMC EMC EMC",
        b"BX",
        b"q 1 2 cm Q",
    ] {
        assert_eq!(
            update::replace(&document, &resources, old, b"", UpdatePolicy::Specification)
                .unwrap_err()
                .kind,
            ErrorKind::Syntax
        );
    }
    let document = Document::open(
        fixture("classic", "blank.pdf"),
        &OpenOptions {
            limits: Limits {
                operations: 3,
                ..Limits::default()
            },
            ..OpenOptions::default()
        },
    )
    .unwrap();
    assert_eq!(
        update::replace(
            &document,
            &resources,
            b"/Tx BMC EMC q Q",
            b"",
            UpdatePolicy::Specification
        )
        .unwrap_err()
        .kind,
        ErrorKind::Limit
    );
}

#[test]
fn update_spans_and_scope_depth_use_existing_budgets() {
    for (limits, source, message) in [
        (
            Limits {
                document_bytes: 16 * 1024,
                ..Limits::default()
            },
            b"/Tx BMC EMC ".repeat(1025),
            "appearance update storage budget exceeded",
        ),
        (
            Limits {
                depth: 8,
                ..Limits::default()
            },
            [b"q ".repeat(9), b"Q ".repeat(9)].concat(),
            "appearance graphics stack budget exceeded",
        ),
    ] {
        let document = Document::open(
            fixture("classic", "blank.pdf"),
            &OpenOptions {
                limits,
                ..OpenOptions::default()
            },
        )
        .unwrap();
        let error = update::replace(
            &document,
            &Dictionary::new(),
            &source,
            b"",
            UpdatePolicy::Specification,
        )
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit);
        assert_eq!(error.message, message);
    }
}

#[test]
fn unmarked_form_artwork_uses_one_bounded_graph_walk() {
    let document = Document::open(
        fixture(
            "annotation_display",
            "update-art-form-artwork-72-source.pdf",
        ),
        &OpenOptions {
            limits: Limits {
                traversal_work: 2,
                ..Limits::default()
            },
            ..OpenOptions::default()
        },
    )
    .unwrap();
    let mut objects = Dictionary::new();
    objects.insert(Name::new(b"Art"), Value::Reference(ObjectId::new(83, 0)));
    let mut resources = Dictionary::new();
    resources.insert(Name::new(b"XObject"), Value::Dictionary(objects));
    let error = update::replace(
        &document,
        &resources,
        b"/Art Do",
        b"",
        UpdatePolicy::PreserveExisting,
    )
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
    assert_eq!(
        error.message,
        "appearance artwork traversal budget exceeded"
    );
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
    let document = document_for_style();
    let resources = Dictionary::new();
    for content in [
        b"BT /Tx BMC (OLD) Tj EMC ET".as_slice(),
        b"q /Tx BMC Q EMC",
        b"/Tx BMC q EMC Q",
    ] {
        let error = update::replace(
            &document,
            &resources,
            content,
            b"/Tx BMC EMC",
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
    let document = document_for_style();
    let resources = Dictionary::new();
    let policy = UpdatePolicy::PreserveExisting;
    let old = b"q 0.7 G 1 1 30 20 re S BT /F1 10 Tf [(old) 20 (value)] TJ ET Q";
    let text = b"/Tx BMC BT /F1 10 Tf (new) Tj ET EMC";
    let actual = update::replace(&document, &resources, old, text, policy).unwrap();
    assert_eq!(
        actual,
        b"q\nq 0.7 G 1 1 30 20 re S BT /F1 10 Tf  ET Q\nQ\n/Tx BMC BT /F1 10 Tf (new) Tj ET EMC"
    );
    for content in [
        b"BT 4 Tr (clip) Tj ET".as_slice(),
        b"/Other Do",
        b"BT (unfinished) Tj",
    ] {
        assert!(update::replace(&document, &resources, content, text, policy).is_err());
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
fn filled_widgets_generate_missing_appearances_and_preserve_identity() {
    let document =
        Document::open(fixture("forms", "render-text.pdf"), &OpenOptions::default()).unwrap();
    let mut filler = document.form_filler().unwrap().unwrap();
    let dictionary = filler.form().widgets[0].dictionary;
    let rect = filler.form().widgets[0].rectangle;
    let fonts = ProviderFonts::new(None, &document.limits());
    let never = |_: &Dictionary| -> Result<bool> { panic!("a missing appearance is not culled") };
    let widgets = FilledWidgets::new(&filler).unwrap();
    assert_eq!(
        widgets.appearance(dictionary, rect, &fonts, never).unwrap(),
        Some(prepare_widget(&filler, 0, None).unwrap())
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
