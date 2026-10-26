//! One source/preview/save contract against independently derived Type 3 pixels.

use super::*;
use pagura_testkit::{Json, Manifest};
use std::sync::Arc;

#[test]
fn source_widgets_untouched_previews_and_saved_reopens_share_layout() {
    let manifest = Manifest::load("annotation_display");
    for case in manifest
        .cases()
        .filter(|case| matches!(case.string("widget_display"), Some("same" | "regenerate")))
    {
        let name = case.name();
        let original = fixture("annotation_display", &format!("{name}.pdf"));
        let document = Arc::new(open(original.clone()).unwrap());
        let context = pagura::render_context(Arc::clone(&document));
        let page = document.page(0).unwrap();
        let options = RenderOptions::from_dpi(case.get("dpi").and_then(Json::as_f64).unwrap());
        let source = pagura::render_page(&document, page, &options).unwrap();
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

        let filler = document.form_filler().unwrap().unwrap();
        let scene = context.filled_scene(&filler, page).unwrap();
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
        if case.string("widget_display") == Some("same") {
            assert_eq!(preview, actual, "{name}: source/untouched preview");
        }
        let saved = save_filled_form(&filler, &SaveOptions::default()).unwrap();
        if !filler.form().need_appearances {
            assert_eq!(saved, original, "{name}: no-op save changed source bytes");
        } else {
            assert!(saved.starts_with(&original), "{name}");
        }
        let reopened = open(saved.clone()).unwrap();
        let after = pagura::render_page(&reopened, reopened.page(0).unwrap(), &options).unwrap();
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
