use super::*;
use pagura_core::{ErrorKind, Limits, syntax::Origin, syntax::Parser};
use pagura_document::OpenOptions;
use pagura_testkit::fixture;

fn dictionary(text: &[u8]) -> Dictionary {
    Parser::new(text, Origin::Buffer, Limits::default())
        .parse()
        .unwrap()
        .as_dictionary()
        .unwrap()
        .clone()
}

fn document() -> Document {
    Document::open(
        fixture("forms", "text-quadding-0.pdf"),
        &OpenOptions::default(),
    )
    .unwrap()
}

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Rect {
    Rect::from_corners(x0, y0, x1, y1).unwrap()
}

fn generate(text: &[u8], area: Rect) -> Result<Option<AppearanceStream>> {
    annotation_appearance(&document(), &dictionary(text), area)
}

#[test]
fn a_link_border_is_a_stroke_inside_its_rectangle_in_default_user_space() {
    let stream = generate(
        b"<< /Subtype /Link /BS << /W 2 /S /D /D [3 1] >> /C [0 0 1] /CA 0.5 >>",
        rect(10.0, 20.0, 30.0, 25.0),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        String::from_utf8_lossy(stream.content()),
        "/StrokeAlpha gs\n0 0 1 RG\n2 w\n[3 1] 0 d\n11 21 18 3 re\nS\n"
    );
    let form = stream.dictionary();
    assert!(form.is_name(b"Subtype", b"Form"));
    assert!(!form.contains_key(b"Matrix"));
    assert_eq!(
        form.get(b"BBox")
            .and_then(Value::as_array)
            .and_then(Rect::from_array),
        Some(rect(10.0, 20.0, 30.0, 25.0))
    );
    let states = form
        .get(b"Resources")
        .and_then(Value::as_dictionary)
        .and_then(|r| r.get(b"ExtGState"))
        .and_then(Value::as_dictionary)
        .unwrap();
    let alpha = states
        .get(b"StrokeAlpha")
        .and_then(Value::as_dictionary)
        .unwrap();
    assert_eq!(alpha.get(b"CA"), Some(&Value::Real(0.5)));
    assert_eq!(states.len(), 1);
}

#[test]
fn opaque_marks_need_no_graphics_state_but_keep_their_own_resources() {
    let stream = generate(
        b"<< /Subtype /Square /IC [1 0 0] /Border [0 0 0] >>",
        rect(0.0, 0.0, 8.0, 4.0),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        String::from_utf8_lossy(stream.content()),
        "1 0 0 rg\n0 0 8 4 re\nf\n"
    );
    // An empty Resources keeps page defaults such as DefaultRGB out.
    let resources = stream
        .dictionary()
        .get(b"Resources")
        .and_then(Value::as_dictionary)
        .unwrap();
    assert!(resources.is_empty());
}

#[test]
fn fill_opacity_falls_back_to_stroke_opacity_and_is_read_only_for_fills() {
    let stream = generate(
        b"<< /Subtype /Square /IC [0.5] /CA 0.25 >>",
        rect(0.0, 0.0, 8.0, 8.0),
    )
    .unwrap()
    .unwrap();
    let content = String::from_utf8_lossy(stream.content()).into_owned();
    assert!(
        content.starts_with("/StrokeAlpha gs\n/FillAlpha gs\n0.5 g\n"),
        "{content}"
    );
    // `ca` is never read without a fill, so a bad one is no error.
    assert!(generate(b"<< /Subtype /Link /ca /Bad >>", rect(0.0, 0.0, 8.0, 8.0)).is_ok());
    let error = generate(
        b"<< /Subtype /Square /IC [0] /ca /Bad >>",
        rect(0.0, 0.0, 8.0, 8.0),
    )
    .unwrap_err();
    assert_eq!(error.message, "annotation parameter must be numeric");
}

#[test]
fn note_icons_are_scaled_from_their_16_unit_design_space() {
    let stream = generate(
        b"<< /Subtype /Text /Name /Insert >>",
        rect(4.0, 8.0, 12.0, 16.0),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        String::from_utf8_lossy(stream.content()),
        "0.5 0 0 0.5 4 8 cm\n1 1 0 rg\n1 1 14 14 re\nf\n0 G\n1 w\n[] 0 d\n\
         1 1 14 14 re\n3 4 m\n8 12 l\n13 4 l\nS\n"
    );
}

#[test]
fn nothing_is_generated_where_nothing_is_drawn() {
    let area = rect(0.0, 0.0, 8.0, 8.0);
    for text in [
        b"<< /Subtype /Popup >>".as_slice(),
        b"<< /Subtype /Projection >>",
        b"<< /Subtype /Link /C [] >>",
        b"<< /Subtype /Link /BS << /W 0 >> >>",
        b"<< /Subtype /Link /Border [0 0 0] >>",
        b"<< /Subtype /Square /Border [0 0 0] >>",
        b"<< /Subtype /Widget /FT /Tx >>",
    ] {
        assert_eq!(
            generate(text, area).unwrap(),
            None,
            "{}",
            String::from_utf8_lossy(text)
        );
    }
    let point = rect(3.0, 3.0, 3.0, 3.0);
    assert_eq!(generate(b"<< /Subtype /Circle >>", point).unwrap(), None);
}

#[test]
fn unsupported_annotations_are_tagged() {
    let area = rect(0.0, 0.0, 8.0, 8.0);
    for text in [
        b"<< /Subtype /Circle >>".as_slice(),
        b"<< /Subtype /Square /BE << /S /C >> >>",
        b"<< /Subtype /Link /Border [2 2 1] >>",
        b"<< /Subtype /Link /BS << /S /I >> >>",
        b"<< /Subtype /Widget /FT /Tx /V (value) >>",
        b"<< /Subtype /Widget /FT /Ch >>",
        b"<< /Subtype /Widget /FT /Tx /MK << /BC [0] /R 90 >> >>",
    ] {
        let error = generate(text, area).unwrap_err();
        assert_eq!(
            error.kind,
            ErrorKind::Unsupported(features::ANNOTATION_APPEARANCE),
            "{}",
            String::from_utf8_lossy(text)
        );
    }
    let error = generate(b"<< /Rect [0 0 1 1] >>", area).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Syntax);
}

#[test]
fn content_and_storage_budgets_bound_generation() {
    let document = Document::open(
        fixture("forms", "text-quadding-0.pdf"),
        &OpenOptions {
            limits: Limits {
                stream_bytes: 16,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    let error = annotation_appearance(
        &document,
        &dictionary(b"<< /Subtype /Text >>"),
        rect(0.0, 0.0, 8.0, 8.0),
    )
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
}
