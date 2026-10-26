use super::*;
use crate::{ProviderFonts, annotation_appearance_with_fonts};
use pagura_core::{
    ErrorKind, Rect,
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
    )
}

fn base() -> Style {
    Style::new("Helvetica".into(), 12.0, Justification::Left)
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
    assert_eq!(style::length("25.4mm", 12.0).unwrap(), 72.0);
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
