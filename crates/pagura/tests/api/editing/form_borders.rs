use super::*;
use pagura_testkit::{Json, Manifest};
use std::sync::Arc;

#[test]
fn filled_border_pixels_survive_save_and_strict_reopen() {
    let manifest = Manifest::load("form_borders");
    assert_eq!(manifest.cases().len(), 35);
    for case in manifest.cases() {
        let name = case.name();
        let original = fixture("form_borders", &format!("{name}.pdf"));
        let document = Arc::new(open(original.clone()).unwrap());
        let context = pagura::render_context(Arc::clone(&document));
        let mut filler = document.form_filler().unwrap().unwrap();
        let input = match case.string("field_kind").unwrap() {
            "Tx" => FillValue::Text(case.string("text").unwrap()),
            "Ch" => FillValue::Choice(ChoiceSelection::Indices(&[1])),
            "Btn" => FillValue::Button(ButtonValue::Off),
            _ => panic!("unknown form border fixture kind"),
        };
        filler.set(0, input).unwrap();
        let dpi = case.get("dpi").and_then(Json::as_f64).unwrap();
        let options = RenderOptions::from_dpi(dpi);
        let scene = context
            .filled_scene(&filler, document.page(0).unwrap())
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        let rendered = context.render_scene(&scene, &options, None).unwrap();
        let expected_size = if dpi == 72.0 { 32 } else { 64 };
        assert_eq!(
            (rendered.width(), rendered.height()),
            (expected_size, expected_size),
            "{name}"
        );
        let actual = crate::support::ppm(&rendered);
        let expected = fixture("form_borders", &format!("{name}.ppm"));
        assert_eq!(actual.len(), expected.len(), "{name}");
        let worst = actual
            .iter()
            .zip(&expected)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap();
        assert!(worst <= 1, "{name}: independent pixel error {worst}");
        let tile = pagura::Tile {
            x: 3,
            y: 5,
            width: 20,
            height: 19,
        };
        let partial = context.render_scene(&scene, &options, Some(tile)).unwrap();
        crate::support::assert_tile_matches(&rendered, &partial, tile);
        let saved = save_filled_form(&filler, &SaveOptions::default())
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert!(saved.starts_with(&original), "{name}");
        let reopened = open(saved.clone()).unwrap();
        let after = pagura::render_page(&reopened, reopened.page(0).unwrap(), &options).unwrap();
        assert_eq!(
            crate::support::ppm(&after),
            actual,
            "{name}: saved appearance differs"
        );
        exported(&format!("border-{name}"), &saved, &original).unwrap();
    }
}
