use super::TextObject;
use super::*;

fn source(text: Option<&str>) -> PositionedText {
    let glyph = PositionedGlyph {
        unicode: text.map(GlyphText::from),
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
        render_mode: TextRenderingMode::Invisible,
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
        objects: Vec::new(),
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
    text.limits.text_work = 250_000;
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
            text_work: 1,
            ..Default::default()
        },
        Limits {
            text_items: 0,
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
        container: None,
        mcid: None,
        actual_text: Some("two\nlines".to_owned()),
        alt: None,
    });
    let source = PositionedText {
        objects: Vec::new(),
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

#[test]
fn retained_bytes_follow_capacity_arithmetic_and_shared_paths_once() -> Result<()> {
    let mut input = source(Some("AB"));
    input.glyphs.reserve_exact(13);
    input.fonts.reserve_exact(5);
    input.objects.reserve_exact(7);
    let mark = Arc::new(MarkedContent {
        id: 0,
        tag: Name::new(b"Span"),
        mcid: None,
        container: None,
        actual_text: Some(String::with_capacity(37)),
        alt: None,
    });
    let path: Arc<[Arc<MarkedContent>]> = Arc::from([Arc::clone(&mark)]);
    input.glyphs[0].marked_content = Arc::clone(&path);
    input.glyphs.push(input.glyphs[0].clone());
    input.marked.push(MarkedSpan {
        content: mark,
        ancestors: Arc::clone(&path),
        glyphs: 0..2,
        hidden: false,
    });
    let text = TextPage::from_positioned(input, LayoutOptions::default(), Limits::default())?;
    let p = &text.positioned;
    let expected = size_of::<PositionedText>()
        + 2 * size_of::<usize>()
        + p.fonts.capacity() * size_of::<TextFont>()
        + p.glyphs.capacity() * size_of::<PositionedGlyph>()
        + p.marked.capacity() * size_of::<MarkedSpan>()
        + p.objects.capacity() * size_of::<TextObject>()
        + text.content.capacity() * size_of::<Character>()
        + text.reading.capacity() * size_of::<Character>()
        + text.lines.capacity() * size_of::<TextLine>()
        + text.words.capacity() * size_of::<TextWord>()
        + text.text.capacity()
        + text.content_text.capacity()
        + p.glyphs
            .iter()
            .map(|g| g.unicode.as_ref().map_or(0, GlyphText::heap_bytes))
            .sum::<usize>()
        + 2 * size_of::<usize>()
        + size_of::<Arc<MarkedContent>>()
        + 2 * size_of::<usize>()
        + size_of::<MarkedContent>()
        + 37;
    assert_eq!(text.retained_bytes(), expected);
    assert_eq!(text.retained_bytes(), memory::estimate(&text));
    let cloned = text.clone();
    assert_eq!(cloned.retained_bytes(), memory::estimate(&cloned));
    assert!(cloned.retained_bytes() <= text.retained_bytes());
    let relaid = text.with_layout(LayoutOptions::default())?;
    assert_eq!(relaid.retained_bytes(), memory::estimate(&relaid));
    Ok(())
}

#[test]
fn retained_bytes_include_long_names_empty_arc_paths_and_shared_glyph_text() -> Result<()> {
    let mut input = source(Some("X"));
    input.fonts[0].base_name = Some(Name::new(&[b'A'; 43]));
    let text = TextPage::from_positioned(input, LayoutOptions::default(), Limits::default())?;
    let base = page("X")?;
    assert_eq!(text.retained_bytes() - base.retained_bytes(), 43);
    let long = "X".repeat(40);
    let shared = source(Some(&long));
    let mut unknown = shared.clone();
    unknown.glyphs[0].unicode = None;
    let heap = GlyphText::from(long.as_str()).heap_bytes();
    assert_ne!(heap, 0);
    assert_eq!(
        memory::source_bytes(&shared) - memory::source_bytes(&unknown),
        heap
    );
    let empty = TextPage::from_positioned(
        PositionedText::default(),
        LayoutOptions::default(),
        Limits::default(),
    )?;
    assert_eq!(
        empty.retained_bytes(),
        size_of::<PositionedText>() + 2 * size_of::<usize>()
    );
    Ok(())
}

#[test]
fn structure_work_budget_and_document_identity_are_enforced()
-> pagura_document::source::ReadResult<()> {
    use pagura_document::{Document, OpenOptions};
    let bytes = pagura_testkit::fixture("text", "tagged-nested-order.pdf");
    let doc = Document::open(bytes.clone(), &OpenOptions::default())?;
    let other = Document::open(bytes.clone(), &OpenOptions::default())?;
    assert_eq!(
        page("X")?
            .with_structure(&other, doc.page(0)?)
            .err()
            .map(|e| e.as_error().unwrap().kind),
        Some(pagura_core::ErrorKind::InvalidArgument)
    );
    let limited = Document::open(
        bytes,
        &OpenOptions {
            limits: Limits {
                text_work: 1,
                ..Default::default()
            },
            ..Default::default()
        },
    )?;
    let text = TextPage::from_positioned(
        PositionedText::default(),
        LayoutOptions::default(),
        Limits::default(),
    )?;
    let error = text.with_structure(&limited, limited.page(0)?).err();
    assert_eq!(
        error.map(|e| e.as_error().unwrap().kind),
        Some(pagura_core::ErrorKind::Limit)
    );
    Ok(())
}

#[test]
fn structure_metadata_capacity_and_heap_roles_are_accounted() -> Result<()> {
    let mut text = page("X")?;
    let base = text.retained_bytes();
    let clone_base = text.clone().retained_bytes();
    let mut elements = Vec::with_capacity(11);
    elements.push(StructureElement {
        object: None,
        parent: None,
        role: Name::new(&[b'X'; 47]),
        standard_type: Some(Name::new(b"P")),
        namespace: StructureNamespace::Pdf17,
        range: 0..1,
    });
    let mut value = String::with_capacity(101);
    value.push('X');
    let mut characters = Vec::with_capacity(17);
    characters.push(text.characters()[0].clone());
    let logical = LogicalText {
        text: value,
        characters,
        structure: elements,
        retained: 0,
    };
    let extra = size_of::<LogicalText>()
        + 2 * size_of::<usize>()
        + logical.text.capacity()
        + logical.characters.capacity() * size_of::<Character>()
        + logical.structure.capacity() * size_of::<StructureElement>()
        + 47;
    let mut logical = logical;
    logical.retained = memory::logical_bytes(&logical);
    assert_eq!(logical.retained, extra);
    let expected = base + extra;
    text.logical = Some(Arc::new(logical));
    text.retained = memory::estimate(&text);
    assert_eq!(text.retained_bytes(), expected);
    assert_eq!(text.clone().retained_bytes(), clone_base + extra);
    let relaid = text.with_layout(LayoutOptions {
        include_hidden: true,
        space_gap: 1.0,
        ..LayoutOptions::default()
    })?;
    assert!(Arc::ptr_eq(
        text.logical.as_ref().unwrap(),
        relaid.logical.as_ref().unwrap()
    ));
    assert_eq!(relaid.retained_bytes(), memory::estimate(&relaid));
    Ok(())
}

#[test]
fn rendering_modes_match_their_operands() -> Result<()> {
    for bits in 0..8 {
        let mode = TextRenderingMode::try_from(bits)?;
        assert_eq!(TextRenderingMode::from_low_bits(bits), mode);
        assert_eq!(u8::from(mode), bits);
        assert_eq!(mode.clips(), bits >= 4);
        assert_eq!(mode.fills(), matches!(bits % 4, 0 | 2));
        assert_eq!(mode.strokes(), matches!(bits % 4, 1 | 2));
    }
    assert!(TextRenderingMode::try_from(8).is_err());
    Ok(())
}
