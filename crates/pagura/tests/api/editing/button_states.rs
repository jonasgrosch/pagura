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
