use super::*;

fn source(text: Option<&str>) -> PositionedText {
    let glyph = PositionedGlyph {
        unicode: text.map(str::to_owned),
        font: 0,
        size: 10.0,
        writing_mode: WritingMode::Horizontal,
        quad: Quad([
            Point::new(0.0, 0.0),
            Point::new(5.0, 0.0),
            Point::new(5.0, 10.0),
            Point::new(0.0, 10.0),
        ]),
        origin: Point::default(),
        axis: Point::new(10.0, 0.0),
        em: 10.0,
        advance_end: Point::new(5.0, 0.0),
        render_mode: 3,
        hidden: false,
        marked_content: Arc::from([]),
        show: 1,
    };
    PositionedText {
        fonts: vec![TextFont {
            object: None,
            kind: FontKind::Type3,
            base_name: None,
        }],
        glyphs: vec![glyph],
        marked: Vec::new(),
    }
}
fn page(text: &str) -> Result<TextPage> {
    TextPage::from_positioned(
        source(Some(text)),
        LayoutOptions::default(),
        Limits::default(),
    )
}

#[test]
fn expansion_boundaries_overlapping_matches_and_unicode_case() -> Result<()> {
    let text = page("Straße Σςσ Banana")?;
    let mut options = SearchOptions::default();
    assert_eq!(text.find("STRASSE", options)?[0].range, 0..7);
    assert_eq!(text.find("s", options)?.len(), 1);
    assert_eq!(text.find("σ", options)?.len(), 3);
    assert_eq!(text.find("ana", options)?.len(), 2);
    options.whole_words = true;
    assert_eq!(text.find("ana", options)?.len(), 0);
    options.case_insensitive = false;
    assert_eq!(text.find("STRASSE", options)?.len(), 0);
    assert_eq!(text.find("", options)?.len(), 0);
    Ok(())
}
#[test]
fn search_is_linear_and_limits_output_and_query_storage() -> Result<()> {
    let mut text = page(&"a".repeat(40000))?;
    text.limits.operations = 250_000;
    assert_eq!(
        text.find(&format!("{}b", "a".repeat(2000)), SearchOptions::default())?
            .len(),
        0
    );
    text.limits.document_bytes = 1024;
    assert!(
        text.find(&"a".repeat(100), SearchOptions::default())
            .is_err()
    );
    assert!(text.find("a", SearchOptions::default()).is_err());
    text.limits.string_bytes = 1;
    assert!(text.find("aa", SearchOptions::default()).is_err());
    Ok(())
}
#[test]
fn unknown_unicode_is_a_barrier_and_geometry_and_layout_budgets_are_validated() -> Result<()> {
    let mut input = source(Some("A"));
    let mut unknown = input.glyphs[0].clone();
    unknown.unicode = None;
    input.glyphs.push(unknown);
    input.glyphs.push(input.glyphs[0].clone());
    let text = TextPage::from_positioned(input, LayoutOptions::default(), Limits::default())?;
    assert_eq!(text.text(), "A\u{fffd}A");
    assert_eq!(text.find("AA", SearchOptions::default())?.len(), 0);
    assert_eq!(text.find("\u{fffd}", SearchOptions::default())?.len(), 0);
    let mut input = source(Some("A"));
    input.glyphs[0].axis.x = f64::NAN;
    assert!(TextPage::from_positioned(input, LayoutOptions::default(), Limits::default()).is_err());
    assert!(
        TextPage::from_positioned(
            source(Some("A")),
            LayoutOptions {
                space_gap: f64::INFINITY,
                ..Default::default()
            },
            Limits::default()
        )
        .is_err()
    );
    for limits in [
        Limits {
            document_bytes: 1,
            ..Default::default()
        },
        Limits {
            string_bytes: 1,
            ..Default::default()
        },
        Limits {
            operations: 1,
            ..Default::default()
        },
        Limits {
            value_nodes: 0,
            ..Default::default()
        },
    ] {
        assert!(
            TextPage::from_positioned(source(Some("AB")), LayoutOptions::default(), limits)
                .is_err()
        );
    }
    Ok(())
}
#[test]
fn empty_and_graphical_replacements_and_literal_line_breaks_are_retained() -> Result<()> {
    let content = Arc::new(MarkedContent {
        id: 0,
        tag: Name::new(b"Span"),
        mcid: None,
        actual_text: Some("two\nlines".to_owned()),
        alt: None,
    });
    let source = PositionedText {
        marked: vec![MarkedSpan {
            content,
            ancestors: Arc::from([]),
            glyphs: 0..0,
            hidden: false,
        }],
        ..Default::default()
    };
    let text = TextPage::from_positioned(source, LayoutOptions::default(), Limits::default())?;
    assert_eq!(text.text(), "two\nlines");
    assert_eq!(text.lines().len(), 2);
    let found = text.find("two lines", SearchOptions::default())?;
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].quads.len(), 0);
    Ok(())
}
