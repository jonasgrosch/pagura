//! One source/preview/save contract against independent text and icon pixels.

use super::*;
use pagura_testkit::{Json, Manifest};
use std::sync::Arc;

#[test]
fn source_widgets_untouched_previews_and_saved_reopens_share_appearance() {
    let manifest = Manifest::load("annotation_display");
    for case in manifest.cases().filter(|case| {
        matches!(case.string("widget_display"), Some("same" | "regenerate"))
            || case.string("button_display") == Some("same")
    }) {
        let name = case.name();
        let original = fixture("annotation_display", &format!("{name}.pdf"));
        let document = Arc::new(open(original.clone()).unwrap());
        let context = pagura::render_context(Arc::clone(&document));
        let page = document.page(0).unwrap();
        let options = RenderOptions::from_dpi(case.get("dpi").and_then(Json::as_f64).unwrap());
        let source = pagura::render_page(&document, page, &options)
            .unwrap_or_else(|error| panic!("{name}: source render: {error}"));
        let expected = fixture("annotation_display", &format!("{name}.ppm"));
        let actual = crate::support::ppm(&source);
        let tolerance = case.get("tolerance").and_then(Json::as_f64).unwrap();
        assert_eq!(actual.len(), expected.len(), "{name}");
        let worst = actual
            .iter()
            .zip(&expected)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap();
        assert!(
            f64::from(worst) <= tolerance,
            "{name}: source pixel error {worst}"
        );

        let filler = document
            .form_filler()
            .unwrap_or_else(|error| panic!("{name}: form view: {error}"))
            .unwrap();
        let scene = context
            .filled_scene(&filler, page)
            .unwrap_or_else(|error| panic!("{name}: untouched preview: {error}"));
        let preview = crate::support::ppm(&context.render_scene(&scene, &options, None).unwrap());
        let expected_preview = fixture(
            "annotation_display",
            &format!("{}.ppm", case.string("widget_preview").unwrap_or(name)),
        );
        assert_eq!(preview.len(), expected_preview.len(), "{name}");
        let worst = preview
            .iter()
            .zip(&expected_preview)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap();
        // Generated text has the same predeclared one-level coverage bound as
        // the missing-AP controls, including when stored artwork was exact.
        assert!(worst <= 1, "{name}: untouched preview pixel error {worst}");
        if case.string("widget_display") == Some("same")
            || case.string("button_display") == Some("same")
        {
            assert_eq!(preview, actual, "{name}: source/untouched preview");
        }
        let saved = save_filled_form(&filler, &SaveOptions::default())
            .unwrap_or_else(|error| panic!("{name}: save: {error}"));
        if filler.form().need_appearances {
            assert!(saved.starts_with(&original), "{name}");
        } else {
            assert_eq!(saved, original, "{name}: no-op save changed source bytes");
        }
        let reopened = open(saved.clone()).unwrap();
        let after = pagura::render_page(&reopened, reopened.page(0).unwrap(), &options)
            .unwrap_or_else(|error| panic!("{name}: reopened render: {error}"));
        assert_eq!(
            crate::support::ppm(&after),
            preview,
            "{name}: saved preview"
        );
        assert!(
            !reopened.acro_form().unwrap().unwrap().need_appearances,
            "{name}"
        );
        assert_eq!(
            document.original_bytes(),
            original,
            "{name}: source mutation"
        );
        if filler.form().need_appearances {
            exported(name, &saved, &original).unwrap();
        }
    }
}

#[test]
fn explicitly_saving_source_values_persists_the_same_missing_appearance() {
    for (name, input) in [
        ("widget-observe-text-72-missing", FillValue::Text("AB")),
        (
            "widget-observe-text-multiline-72-missing",
            FillValue::Text("AB\nCD"),
        ),
        ("widget-observe-text-comb-72-missing", FillValue::Text("AB")),
        (
            "widget-observe-text-inherited-72-missing",
            FillValue::Text("AB"),
        ),
        (
            "widget-observe-text-null-inheritance-72-missing",
            FillValue::Text("AB"),
        ),
        ("widget-source-separate", FillValue::Text("AB")),
        ("widget-source-siblings", FillValue::Text("AB")),
        (
            "widget-observe-text-widget-rotation-72-missing",
            FillValue::Text("AB"),
        ),
        (
            "widget-observe-combo-72-missing",
            FillValue::Choice(ChoiceSelection::Indices(&[1])),
        ),
        (
            "widget-observe-combo-editable-72-missing",
            FillValue::Choice(ChoiceSelection::Text("CUSTOM")),
        ),
        (
            "widget-observe-list-multi-72-missing",
            FillValue::Choice(ChoiceSelection::Indices(&[0, 2])),
        ),
        (
            "widget-observe-list-top-72-missing",
            FillValue::Choice(ChoiceSelection::Indices(&[2])),
        ),
    ] {
        let original = fixture("annotation_display", &format!("{name}.pdf"));
        let document = Arc::new(open(original.clone()).unwrap());
        let mut filler = document.form_filler().unwrap().unwrap();
        let field = filler.form().widgets[0].field;
        let old_value = filler.form().fields[field].value.clone();
        filler.set(field, input).unwrap();
        let context = pagura::render_context(Arc::clone(&document));
        let scene = context
            .filled_scene(&filler, document.page(0).unwrap())
            .unwrap();
        let options = RenderOptions::from_dpi(72.0);
        let preview = crate::support::ppm(&context.render_scene(&scene, &options, None).unwrap());
        let expected = fixture("annotation_display", &format!("{name}.ppm"));
        assert_eq!(preview.len(), expected.len(), "{name}");
        let worst = preview
            .iter()
            .zip(&expected)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap();
        assert!(worst <= 1, "{name}: saved source-value pixel error {worst}");
        let saved = save_filled_form(&filler, &SaveOptions::default()).unwrap();
        assert!(saved.starts_with(&original), "{name}");
        let reopened = open(saved.clone()).unwrap();
        let form = reopened.acro_form().unwrap().unwrap();
        assert!(form.widgets[0].dictionary.get(b"AP").is_some(), "{name}");
        let after = pagura::render_page(&reopened, reopened.page(0).unwrap(), &options).unwrap();
        assert_eq!(crate::support::ppm(&after), preview, "{name}: persisted AP");
        assert_eq!(
            filler.form().fields[field].value,
            old_value,
            "{name}: source V mutation"
        );
        exported(&format!("saved-{name}"), &saved, &original).unwrap();
    }
}

#[test]
fn rich_replacements_share_source_layout_and_persist_plain_and_rich_values() {
    let manifest = Manifest::load("annotation_display");
    for case in manifest
        .cases()
        .filter(|case| case.string("rich_input").is_some())
    {
        let name = case.name();
        let xml = case.string("rich_input").unwrap();
        let expected_value = case.string("rich_plain").unwrap();
        let original = fixture("annotation_display", &format!("{name}.pdf"));
        let document = Arc::new(open(original.clone()).unwrap());
        let mut filler = document.form_filler().unwrap().unwrap();
        let field = filler.form().widgets[0].field;
        let old_default = filler.form().fields[field].default_value.clone();
        let old_style = filler.form().fields[field].dictionary.get(b"DS").cloned();
        filler
            .set(
                field,
                FillValue::RichText {
                    xml,
                    default_style: None,
                },
            )
            .unwrap();
        assert_eq!(
            pagura::form_appearance::rich_text_value(&filler, field).unwrap(),
            expected_value,
            "{name}"
        );
        let context = pagura::render_context(Arc::clone(&document));
        let scene = context
            .filled_scene(&filler, document.page(0).unwrap())
            .unwrap_or_else(|error| panic!("{name}: rich preview: {error}"));
        let options = RenderOptions::from_dpi(case.get("dpi").and_then(Json::as_f64).unwrap());
        let preview = crate::support::ppm(&context.render_scene(&scene, &options, None).unwrap());
        let oracle = case.string("widget_preview").unwrap_or(name);
        let expected = fixture("annotation_display", &format!("{oracle}.ppm"));
        assert_eq!(preview.len(), expected.len(), "{name}");
        let worst = preview
            .iter()
            .zip(&expected)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap();
        assert!(worst <= 1, "{name}: rich replacement pixel error {worst}");
        let saved = save_filled_form(&filler, &SaveOptions::default())
            .unwrap_or_else(|error| panic!("{name}: rich save: {error}"));
        assert!(saved.starts_with(&original), "{name}");
        let reopened = open(saved.clone()).unwrap();
        let form = reopened.acro_form().unwrap().unwrap();
        let after = &form.fields[field];
        assert_eq!(
            after.value,
            Some(FieldValue::Text(expected_value.to_owned())),
            "{name}"
        );
        assert_eq!(after.default_value, old_default, "{name}: default changed");
        assert_eq!(
            after.dictionary.get(b"DS"),
            old_style.as_ref(),
            "{name}: source DS changed"
        );
        assert_eq!(
            after.dictionary.get(b"RV"),
            Some(&Value::String(pagura_core::text_string::encode(xml))),
            "{name}"
        );
        assert_eq!(
            crate::support::ppm(
                &pagura::render_page(&reopened, reopened.page(0).unwrap(), &options).unwrap()
            ),
            preview,
            "{name}: reopened rich appearance"
        );
        assert_eq!(
            document.original_bytes(),
            original,
            "{name}: source mutation"
        );
        filler.reset(field);
        if !filler.form().need_appearances {
            assert_eq!(
                save_filled_form(&filler, &SaveOptions::default()).unwrap(),
                original,
                "{name}: reset changed bytes"
            );
        }
        exported(&format!("saved-{name}"), &saved, &original).unwrap();
    }
}

#[test]
fn rich_replacements_can_replace_unused_bad_payloads_and_rebuild_the_whole_form() {
    let original = fixture("annotation_display", "rich-field-lazy-stored.pdf");
    let document = Arc::new(open(original.clone()).unwrap());
    let mut filler = document.form_filler().unwrap().unwrap();
    let xml = "<body><p><span style=\"color:#ff0000\">A</span>B</p></body>";
    filler
        .set(
            0,
            FillValue::RichText {
                xml,
                default_style: Some("font-size:10pt;color:#0000ff"),
            },
        )
        .unwrap();
    let saved = save_filled_form(&filler, &SaveOptions::default()).unwrap();
    let reopened = open(saved).unwrap();
    let form = reopened.acro_form().unwrap().unwrap();
    assert_eq!(
        form.fields[0].dictionary.get(b"DS"),
        Some(&Value::String(b"font-size:10pt;color:#0000ff".to_vec()))
    );
    assert_eq!(
        form.fields[0].value,
        Some(FieldValue::Text("AB".to_owned()))
    );
    let actual = pagura::render_page(
        &reopened,
        reopened.page(0).unwrap(),
        &RenderOptions::from_dpi(72.0),
    )
    .unwrap();
    assert_eq!(
        crate::support::ppm(&actual),
        fixture("annotation_display", "rich-field-styled-72-source.ppm")
    );
    assert_eq!(document.original_bytes(), original);

    let document = open(fixture(
        "annotation_display",
        "rich-field-need-regenerate.pdf",
    ))
    .unwrap();
    let filler = document.form_filler().unwrap().unwrap();
    let stream = pagura::form_appearance::prepare_widget(&filler, 0, None).unwrap();
    let pagura::form_appearance::Appearance::Generated(stream) = stream else {
        panic!("expected regenerated rich appearance")
    };
    assert_eq!(
        stream.dictionary().get(b"BBox"),
        Some(&Value::Array(vec![
            Value::Real(0.0),
            Value::Real(0.0),
            Value::Real(48.0),
            Value::Real(32.0)
        ]))
    );
    assert!(
        !stream
            .content()
            .windows(b"0 0 24 16 re f".len())
            .any(|v| v == b"0 0 24 16 re f")
    );
    let saved = save_filled_form(&filler, &SaveOptions::default()).unwrap();
    let reopened = open(saved).unwrap();
    let form = reopened.acro_form().unwrap().unwrap();
    let ap = form.widgets[0]
        .dictionary
        .get(b"AP")
        .unwrap()
        .as_dictionary()
        .unwrap();
    assert_eq!(ap.get(b"R"), Some(&Value::Reference(ObjectId::new(81, 0))));
    assert_eq!(ap.get(b"D"), Some(&Value::Reference(ObjectId::new(81, 0))));
    assert!(matches!(
        form.fields[0].kind,
        pagura::forms::FieldKind::Text {
            rich_value: Some(_),
            ..
        }
    ));
    assert!(!form.need_appearances);
}

#[test]
fn rich_selected_errors_leave_source_bytes_intact_and_passwords_unpersisted() {
    for (xml, style, expected) in [
        ("<body><p>AB</body>", None, ErrorKind::Syntax),
        (
            "<body><p>A</p><p>B</p></body>",
            None,
            ErrorKind::InvalidArgument,
        ),
        ("<body><p>ABC</p></body>", None, ErrorKind::InvalidArgument),
        (
            "<body><p>AB</p></body>",
            Some("margin-left:2em"),
            ErrorKind::Unsupported(pagura_document::features::RICH_TEXT),
        ),
    ] {
        let original = fixture("annotation_display", "rich-field-maxlen.pdf");
        let document = Arc::new(open(original.clone()).unwrap());
        let mut filler = document.form_filler().unwrap().unwrap();
        filler
            .set(
                0,
                FillValue::RichText {
                    xml,
                    default_style: style,
                },
            )
            .unwrap();
        let error = pagura::form_appearance::prepare_widget(&filler, 0, None).unwrap_err();
        assert_eq!(error.kind, expected, "{xml}");
        assert_eq!(
            save_filled_form(&filler, &SaveOptions::default())
                .unwrap_err()
                .kind,
            expected,
            "{xml}"
        );
        assert_eq!(document.original_bytes(), original);
        filler.reset(0);
        assert_eq!(
            save_filled_form(&filler, &SaveOptions::default()).unwrap(),
            original
        );
    }
    let document = open(fixture(
        "annotation_display",
        "rich-field-no-scroll-fit.pdf",
    ))
    .unwrap();
    let mut filler = document.form_filler().unwrap().unwrap();
    filler
        .set(
            0,
            FillValue::RichText {
                xml: "<body><p>ABCDEFGHI</p></body>",
                default_style: None,
            },
        )
        .unwrap();
    assert_eq!(
        pagura::form_appearance::prepare_widget(&filler, 0, None)
            .unwrap_err()
            .kind,
        ErrorKind::InvalidArgument
    );
    assert_eq!(
        save_filled_form(&filler, &SaveOptions::default())
            .unwrap_err()
            .kind,
        ErrorKind::InvalidArgument
    );

    let document = open(fixture("annotation_display", "rich-field-password.pdf")).unwrap();
    let mut filler = document.form_filler().unwrap().unwrap();
    filler
        .set(
            0,
            FillValue::RichText {
                xml: "<body><p>AB</p></body>",
                default_style: None,
            },
        )
        .unwrap();
    let stream = pagura::form_appearance::prepare_widget(&filler, 0, None).unwrap();
    let pagura::form_appearance::Appearance::Generated(stream) = stream else {
        panic!("expected password preview")
    };
    assert!(stream.content().windows(7).any(|bytes| bytes == b"<2A> Tj"));
    assert!(
        !stream
            .content()
            .windows(7)
            .any(|bytes| bytes == b"<41> Tj" || bytes == b"<42> Tj")
    );
    assert_eq!(
        save_filled_form(&filler, &SaveOptions::default())
            .unwrap_err()
            .kind,
        ErrorKind::Unsupported(features::PASSWORD)
    );
}
