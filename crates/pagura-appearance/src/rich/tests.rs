use super::*;
use crate::{ProviderFonts, annotation_appearance_with_fonts};
use pagura_core::{
    ErrorKind, Limits, Name, Rect,
    syntax::{Origin, Parser},
};
use pagura_document::OpenOptions;
use pagura_testkit::fixture;

fn parse(source: &str, base: Style, limits: &Limits) -> Result<xml::Text> {
    xml::Text::parse(
        source,
        base,
        limits,
        &mut Meter::new(limits.operations, "test work"),
        &mut Meter::new(limits.document_bytes, "test storage"),
    )
}

fn base() -> Style {
    Style::new(Name::new(b"Helvetica"), 12.0, Justification::Left)
}

#[test]
fn nested_styles_restore_inheritance_and_preserve_entity_text() {
    let text = parse("<body><p>A <b>B<span style='color:#0000ff;font-size:18pt'>C&#32;</span>D</b> E&amp;F</p></body>",base(),&Limits::default()).unwrap();
    assert_eq!(
        text.characters.iter().map(|c| c.value).collect::<String>(),
        "A BC D E&F"
    );
    assert_eq!(text.paragraphs.len(), 1);
    assert!(!text.styles[text.characters[0].style].bold);
    assert!(text.styles[text.characters[2].style].bold);
    assert_eq!(text.styles[text.characters[3].style].size, 18.0);
    assert_eq!(
        text.styles[text.characters[3].style].color,
        Some([0.0, 0.0, 1.0])
    );
    assert!(!text.styles[text.characters[7].style].bold);
}

#[test]
fn xml_nesting_entities_and_external_objects_fail_explicitly() {
    for xml in [
        "text",
        "<p>text",
        "<p></b>",
        "<p>&unknown;</p>",
        "<p>&#0;</p>",
        "<p><p>x</p></p>",
        "<p/><p/>",
    ] {
        assert_eq!(
            parse(xml, base(), &Limits::default()).err().unwrap().kind,
            ErrorKind::Syntax,
            "{xml}"
        );
    }
    for xml in ["<!DOCTYPE p><p/>", "<p><span xfa:embed='anything'/></p>"] {
        assert_eq!(
            parse(xml, base(), &Limits::default()).err().unwrap().kind,
            ErrorKind::Unsupported(features::ANNOTATION_APPEARANCE)
        );
    }
}

#[test]
fn syntax_depth_nodes_and_storage_have_independent_budgets() {
    for limits in [
        Limits {
            depth: 1,
            ..Limits::default()
        },
        Limits {
            value_nodes: 2,
            ..Limits::default()
        },
        Limits {
            document_bytes: 512,
            ..Limits::default()
        },
    ] {
        assert_eq!(
            parse("<body><p>AB</p></body>", base(), &limits)
                .err()
                .unwrap()
                .kind,
            ErrorKind::Limit
        );
    }
}

#[test]
fn default_styles_parse_units_shorthand_and_paragraph_margins() {
    let mut style = base();
    style.apply("font: italic bold 10pt/14pt Helvetica;margin:1pt 2pt 3pt 4pt;color:rgb(0,0,255);text-align:right;vertical-align:2pt").unwrap();
    assert_eq!(style.size, 10.0);
    assert_eq!(style.leading, Some(14.0));
    assert_eq!(style.margin, [1.0, 2.0, 3.0, 4.0]);
    assert!(style.bold && style.italic);
    assert_eq!(style.rise, 2.0);
    assert_eq!(style.alignment, Justification::Right);
    assert_eq!(style::length("25.4mm").unwrap(), 72.0);
}

#[test]
fn rich_annotations_use_private_resources_and_ignore_unused_contents() {
    let document =
        Document::open(fixture("classic", "rectangle.pdf"), &OpenOptions::default()).unwrap();
    let annotation=Parser::new(b"<< /Subtype /FreeText /DA (/Helvetica 12 Tf) /Contents 42 /RC (<body><p><b>HI</b></p></body>) /BS << /W 0 >> >>",Origin::Buffer,Limits::default()).parse().unwrap();
    let provider = pagura_data::Bundled::default();
    let fonts = ProviderFonts::new(Some(&provider), &document.limits());
    let stream = annotation_appearance_with_fonts(
        &document,
        annotation.as_dictionary().unwrap(),
        Rect::from_corners(4.0, 4.0, 124.0, 44.0).unwrap(),
        &fonts,
    )
    .unwrap()
    .unwrap();
    let resources = stream
        .dictionary()
        .get(b"Resources")
        .unwrap()
        .as_dictionary()
        .unwrap();
    let fonts = resources.get(b"Font").unwrap().as_dictionary().unwrap();
    assert_eq!(fonts.len(), 1);
    assert_eq!(
        fonts
            .get(b"Rich0")
            .unwrap()
            .as_dictionary()
            .unwrap()
            .get_name(b"BaseFont")
            .unwrap()
            .as_bytes(),
        b"Helvetica-Bold"
    );
}

#[test]
fn shared_caption_layout_preserves_empty_lines_and_ignores_unrelated_ds() {
    let document = Document::open(
        fixture("forms", "text-quadding-0.pdf"),
        &OpenOptions::default(),
    )
    .unwrap();
    let annotation = Parser::new(
        b"<< /Subtype /Line /DS 42 >>",
        Origin::Buffer,
        Limits::default(),
    )
    .parse()
    .unwrap();
    let appearance = DefaultAppearance::parse(b"/F1 10 Tf", &document.limits()).unwrap();
    let fonts = ProviderFonts::new(None, &document.limits());
    // The project-owned font has literal .8/- .2 em metrics and .6 em advances.
    for (text, width, height) in [
        ("A", 6.0, 10.0),
        ("A\n", 6.0, 22.0),
        ("\nA", 6.0, 22.0),
        ("\r\nA\r\n", 6.0, 34.0),
        ("\n", 0.0, 22.0),
    ] {
        let mut annotation = annotation.as_dictionary().unwrap().clone();
        annotation.insert(
            Name::new(b"Contents"),
            Value::String(text_string::encode(text)),
        );
        let layout = prepare(
            &document,
            &annotation,
            &appearance,
            Justification::Center,
            f64::INFINITY,
            &fonts,
            &mut Meter::new(document.limits().document_bytes, "test storage"),
        )
        .unwrap();
        assert_eq!((layout.width, layout.height), (width, height), "{text:?}");
    }
}

#[test]
fn small_rich_appearance_uses_the_shared_storage_allowance() {
    let document = Document::open(
        fixture("rich_annotations", "plain-72.pdf"),
        &OpenOptions {
            limits: Limits {
                document_bytes: 65_536,
                ..Limits::default()
            },
            ..OpenOptions::default()
        },
    )
    .unwrap();
    let annotation = Parser::new(
        b"<< /Subtype /FreeText /DA (/F1 10 Tf) /RC (<p>AB</p>) /BS << /W 0 >> >>",
        Origin::Buffer,
        Limits::default(),
    )
    .parse()
    .unwrap();
    let fonts = ProviderFonts::new(None, &document.limits());
    let appearance = annotation_appearance_with_fonts(
        &document,
        annotation.as_dictionary().unwrap(),
        Rect::from_corners(4.0, 4.0, 124.0, 60.0).unwrap(),
        &fonts,
    )
    .unwrap()
    .unwrap();
    assert!(String::from_utf8_lossy(appearance.content()).contains("<41> Tj"));
}

#[test]
fn field_plain_values_follow_the_same_entity_whitespace_and_paragraph_parser() {
    let text = parse(
        "<body><p>A&#x42;  C</p><p/><p>D<br/>E</p></body>",
        base(),
        &Limits::default(),
    )
    .unwrap();
    let value = text
        .value(
            &Limits::default(),
            &mut Meter::new(128, "test storage"),
            &mut Meter::new(128, "test work"),
        )
        .unwrap();
    assert_eq!(value, "AB C\n\nD\nE");
    let error = text
        .value(
            &Limits {
                string_bytes: 4,
                ..Default::default()
            },
            &mut Meter::new(128, "test storage"),
            &mut Meter::new(128, "test work"),
        )
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
    let error = text
        .value(
            &Limits::default(),
            &mut Meter::new(4, "test storage"),
            &mut Meter::new(128, "test work"),
        )
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
}
