use super::*;

#[test]
fn generated_button_states_survive_off_on_and_repeated_saves() {
    let original = fixture("forms", "render-checkbox-generated.pdf");
    let mut bytes = original.clone();
    for (revision, selected) in [false, true, false, true].into_iter().enumerate() {
        let document = open(bytes.clone()).unwrap();
        let mut filler = document.form_filler().unwrap().unwrap();
        let on = filler.button_on_state(0).unwrap();
        let value = if selected {
            ButtonValue::Widget {
                index: 0,
                state: Some(&on),
            }
        } else {
            ButtonValue::Off
        };
        filler.set(0, FillValue::Button(value)).unwrap();
        let saved = save_filled_form(&filler, &SaveOptions::default()).unwrap();
        assert!(saved.starts_with(&bytes));
        let reopened = open(saved.clone()).unwrap();
        let form = reopened.acro_form().unwrap().unwrap();
        assert_eq!(form.widgets[0].on_states, [&on]);
        let expected = if selected {
            on.clone()
        } else {
            Name::new(b"Off")
        };
        assert_eq!(form.widgets[0].appearance_state, Some(&expected));
        assert_eq!(form.fields[0].value, Some(FieldValue::Name(&expected)));
        let states = reopened
            .resolve_dictionary(form.widgets[0].dictionary.get(b"AP").unwrap())
            .and_then(|ap| reopened.resolve_dictionary(ap.get(b"N").unwrap()))
            .unwrap();
        assert_eq!(states.len(), 2);
        for state in [b"Off".as_slice(), on.as_bytes()] {
            let id = states.get(state).unwrap().as_reference().unwrap();
            assert!(reopened.object(id).unwrap().is_stream());
        }
        exported(&format!("generated-checkbox-{revision}"), &saved, &bytes).unwrap();
        bytes = saved;
    }
    assert!(bytes.starts_with(&original));
}

#[test]
fn generated_on_names_share_the_fillers_source_selection() {
    for (source, expected) in [
        ("/AS /Custom", b"Custom".as_slice()),
        ("/AS /Off /V /Value", b"Value"),
        ("/AS /Off /V /Off /DV /Default", b"Default"),
        ("/Opt [(Export)]", b"0"),
        ("/Opt [(Export)] /AS /Explicit", b"Explicit"),
        ("", b"Yes"),
    ] {
        let annotation = format!(
            "<< /Type /Annot /Subtype /Widget /FT /Btn /T (entry) /Rect [2 2 18 18] {source} >>"
        );
        let bytes = raw_pdf(
            &[
                (
                    1,
                    0,
                    "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] >> >>",
                ),
                (2, 0, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
                (
                    3,
                    0,
                    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 20 20] /Annots [4 0 R] >>",
                ),
                (4, 0, &annotation),
            ],
            " /Root 1 0 R /Size 5",
        );
        let document = open(bytes).unwrap();
        let mut filler = document.form_filler().unwrap().unwrap();
        assert_eq!(filler.button_on_state(0).unwrap().as_bytes(), expected);
        filler.set(0, FillValue::Button(ButtonValue::Off)).unwrap();
        let saved = save_filled_form(&filler, &SaveOptions::default()).unwrap();
        let reopened = open(saved).unwrap();
        let mut again = reopened.form_filler().unwrap().unwrap();
        let name = Name::new(expected);
        again
            .set(0, FillValue::Button(ButtonValue::State(&name)))
            .unwrap();
        assert_eq!(again.button_on_state(0).unwrap(), name);
    }
}

#[test]
fn unselected_state_failure_does_not_publish_a_partial_save() {
    let original = fixture("forms", "render-checkbox-generated.pdf");
    let document = Document::open(
        original.clone(),
        &OpenOptions {
            recovery: Recovery::Strict,
            limits: Limits {
                stream_bytes: 48,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    let mut filler = document.form_filler().unwrap().unwrap();
    filler.set(0, FillValue::Button(ButtonValue::Off)).unwrap();
    assert_eq!(
        save_filled_form(&filler, &SaveOptions::default())
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
    assert_eq!(document.original_bytes(), original);
    assert_eq!(filler.changes().count(), 1);
}

#[test]
fn need_appearances_preserves_untouched_button_selection() {
    for flags in [0, 32768] {
        let annotation = format!(
            "<< /Subtype /Widget /FT /Btn /Ff {flags} /T (entry) /V /Selected /Rect [2 2 18 18] >>"
        );
        let original = raw_pdf(
            &[
                (
                    1,
                    0,
                    "<< /Type /Catalog /Pages 2 0 R /AcroForm << /NeedAppearances true /Fields [4 0 R] >> >>",
                ),
                (2, 0, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
                (
                    3,
                    0,
                    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 20 20] /Annots [4 0 R] >>",
                ),
                (4, 0, &annotation),
            ],
            " /Root 1 0 R /Size 5",
        );
        let document = open(original.clone()).unwrap();
        let filler = document.form_filler().unwrap().unwrap();
        let saved = save_filled_form(&filler, &SaveOptions::default()).unwrap();
        let reopened = open(saved.clone()).unwrap();
        let form = reopened.acro_form().unwrap().unwrap();
        assert!(!form.need_appearances);
        assert_eq!(
            form.widgets[0].appearance_state.unwrap().as_bytes(),
            b"Selected"
        );
        assert_eq!(form.widgets[0].on_states[0].as_bytes(), b"Selected");
        let mut again = reopened.form_filler().unwrap().unwrap();
        again.set(0, FillValue::Button(ButtonValue::Off)).unwrap();
        assert!(
            save_filled_form(&again, &SaveOptions::default())
                .unwrap()
                .starts_with(&saved)
        );
        exported(&format!("generated-button-need-{flags}"), &saved, &original).unwrap();
    }
}

#[test]
fn normal_captions_persist_both_states_and_explicit_edits_override_source_as() {
    for variant in ["check-caption", "radio-caption"] {
        let original = fixture(
            "annotation_display",
            &format!("button-normal-{variant}-72.pdf"),
        );
        let mut bytes = original.clone();
        for (revision, selected) in [false, true, false, true].into_iter().enumerate() {
            let document = open(bytes.clone()).unwrap();
            let mut filler = document.form_filler().unwrap().unwrap();
            let field = filler.form().widgets[0].field;
            filler
                .set(
                    field,
                    FillValue::Button(if selected {
                        ButtonValue::Widget {
                            index: 0,
                            state: None,
                        }
                    } else {
                        ButtonValue::Off
                    }),
                )
                .unwrap();
            let saved = save_filled_form(&filler, &SaveOptions::default())
                .unwrap_or_else(|error| panic!("{variant}: save: {error}"));
            assert!(saved.starts_with(&bytes), "{variant}");
            let reopened = open(saved.clone()).unwrap();
            let form = reopened.acro_form().unwrap().unwrap();
            let widget = &form.widgets[0];
            let ap = reopened
                .resolve_dictionary(widget.dictionary.get(b"AP").unwrap())
                .unwrap();
            let states = reopened.resolve_dictionary(ap.get(b"N").unwrap()).unwrap();
            assert_eq!(states.len(), 2, "{variant}: state persistence");
            assert!(
                states.get(b"Off").is_some() && states.get(b"Yes").is_some(),
                "{variant}"
            );
            let expected = if selected {
                b"Yes".as_slice()
            } else {
                b"Off".as_slice()
            };
            assert_eq!(
                widget.appearance_state.unwrap().as_bytes(),
                expected,
                "{variant}"
            );
            let image = pagura::render_page(
                &reopened,
                reopened.page(0).unwrap(),
                &RenderOptions::from_dpi(72.0),
            )
            .unwrap();
            let oracle = fixture(
                "annotation_display",
                &if selected {
                    format!("button-normal-{variant}-72.ppm")
                } else {
                    "button-normal-check-as-off-72.ppm".to_owned()
                },
            );
            assert_eq!(
                crate::support::ppm(&image),
                oracle,
                "{variant}: reopened caption pixels"
            );
            exported(&format!("normal-{variant}-{revision}"), &saved, &bytes).unwrap();
            bytes = saved;
        }
        assert!(bytes.starts_with(&original), "{variant}");
    }
}

#[test]
fn generating_a_push_appearance_never_creates_a_field_value_or_state() {
    for name in [
        "button-normal-push-caption-72",
        "button-normal-push-need-72",
        "button-normal-push-position-6-save-generated",
    ] {
        let original = fixture("annotation_display", &format!("{name}.pdf"));
        let document = open(original.clone()).unwrap();
        let mut filler = document.form_filler().unwrap().unwrap();
        assert_eq!(
            filler
                .set(0, FillValue::Button(ButtonValue::Off))
                .unwrap_err()
                .kind,
            ErrorKind::InvalidArgument
        );
        let saved = save_filled_form(&filler, &SaveOptions::default()).unwrap();
        if !filler.form().need_appearances {
            assert_eq!(saved, original, "{name}: no-op save");
        }
        let reopened = open(saved).unwrap();
        let form = reopened.acro_form().unwrap().unwrap();
        let field = &form.fields[0];
        assert!(
            field.value.is_none() && field.default_value.is_none(),
            "{name}"
        );
        for key in [b"V".as_slice(), b"DV", b"AS"] {
            assert!(
                field.dictionary.get(key).is_none(),
                "{name}: synthetic value/state"
            );
        }
        if filler.form().need_appearances {
            let ap = reopened
                .resolve_dictionary(form.widgets[0].dictionary.get(b"AP").unwrap())
                .unwrap();
            let normal = ap.get(b"N").unwrap().as_reference().unwrap();
            assert!(
                reopened.object(normal).unwrap().is_stream(),
                "{name}: normal AP must be a stream"
            );
        }
    }
}

#[test]
fn persisting_off_validates_the_unselected_caption_without_partial_state() {
    let original = fixture(
        "annotation_display",
        "button-normal-check-off-ignores-caption.pdf",
    );
    let document = open(original.clone()).unwrap();
    let mut filler = document.form_filler().unwrap().unwrap();
    filler.set(0, FillValue::Button(ButtonValue::Off)).unwrap();
    // Off itself draws no caption; persistence must also prepare the on-state.
    assert!(matches!(
        pagura::form_appearance::prepare_widget(&filler, 0, None).unwrap(),
        pagura::form_appearance::Appearance::Generated(_)
    ));
    assert_eq!(
        save_filled_form(&filler, &SaveOptions::default())
            .unwrap_err()
            .kind,
        ErrorKind::Syntax
    );
    assert_eq!(document.original_bytes(), original);
    assert_eq!(filler.changes().count(), 1);
}

#[test]
fn missing_as_uses_opt_ordinals_and_shared_on_names_preserve_unison() {
    for suffix in [
        "radio-1",
        "unison-1",
        "check-1",
        "null-as",
        "unison-shared-state",
        "check-shared-state",
    ] {
        let name = format!("button-normal-opt-{suffix}");
        let original = fixture("annotation_display", &format!("{name}.pdf"));
        let document = open(original.clone()).unwrap();
        let mut filler = document.form_filler().unwrap().unwrap();
        let shared = suffix.ends_with("shared-state");
        for (index, widget) in filler.form().widgets.iter().enumerate() {
            let state = filler.button_appearance_state(index).unwrap();
            let source = document.widget_field(widget.dictionary).unwrap().unwrap();
            assert_eq!(source.button_state, state, "{name}: source/preview state");
            assert_eq!(
                state.map(Name::as_bytes),
                if shared {
                    Some(b"Shared".as_slice())
                } else if index == 1 {
                    Some(b"1".as_slice())
                } else {
                    None
                },
                "{name}"
            );
        }
        let saved = save_filled_form(&filler, &SaveOptions::default()).unwrap();
        let reopened = open(saved).unwrap();
        let form = reopened.acro_form().unwrap().unwrap();
        for (index, widget) in form.widgets.iter().enumerate() {
            let expected = if shared {
                b"Shared".as_slice()
            } else if index == 1 {
                b"1".as_slice()
            } else {
                b"Off".as_slice()
            };
            assert_eq!(
                widget.appearance_state.unwrap().as_bytes(),
                expected,
                "{name}: saved state"
            );
            let ap = reopened
                .resolve_dictionary(widget.dictionary.get(b"AP").unwrap())
                .unwrap();
            let states = reopened.resolve_dictionary(ap.get(b"N").unwrap()).unwrap();
            assert!(
                states.get(expected).is_some(),
                "{name}: dangling appearance state"
            );
        }
        // Duplicate export strings do not conflate distinct numeric on names;
        // equal explicit on names do turn on together under the existing flags.
        filler
            .set(
                0,
                FillValue::Button(ButtonValue::Widget {
                    index: 0,
                    state: None,
                }),
            )
            .unwrap();
        let saved = save_filled_form(&filler, &SaveOptions::default()).unwrap();
        let edited = open(saved.clone()).unwrap();
        let image = pagura::render_page(
            &edited,
            edited.page(0).unwrap(),
            &RenderOptions::from_dpi(72.0),
        )
        .unwrap();
        let oracle = if shared {
            format!("{name}.ppm")
        } else {
            "button-normal-opt-radio-0.ppm".to_owned()
        };
        assert_eq!(
            crate::support::ppm(&image),
            fixture("annotation_display", &oracle),
            "{name}: explicit selection"
        );
        exported(&name, &saved, &original).unwrap();
    }
}

#[test]
fn ambiguous_exclusive_radio_display_requires_an_explicit_selection() {
    let original = fixture("annotation_display", "button-normal-radio-unidentified.pdf");
    let document = open(original.clone()).unwrap();
    let mut filler = document.form_filler().unwrap().unwrap();
    let error = pagura::render_page(
        &document,
        document.page(0).unwrap(),
        &RenderOptions::default(),
    )
    .unwrap_err();
    assert!(
        matches!(error.kind, ErrorKind::Unsupported(feature) if feature.as_str() == "annotation.appearance")
    );
    assert!(
        matches!(save_filled_form(&filler, &SaveOptions::default()).unwrap_err().kind, ErrorKind::Unsupported(feature) if feature.as_str() == "annotation.appearance")
    );
    assert_eq!(document.original_bytes(), original);
    filler
        .set(
            0,
            FillValue::Button(ButtonValue::Widget {
                index: 0,
                state: None,
            }),
        )
        .unwrap();
    let saved = save_filled_form(&filler, &SaveOptions::default()).unwrap();
    let reopened = open(saved).unwrap();
    let image = pagura::render_page(
        &reopened,
        reopened.page(0).unwrap(),
        &RenderOptions::from_dpi(72.0),
    )
    .unwrap();
    assert_eq!(
        crate::support::ppm(&image),
        fixture("annotation_display", "button-normal-opt-radio-0.ppm")
    );
}
