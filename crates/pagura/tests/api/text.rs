#![expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "independent fixture assertions fail loudly on malformed expected data"
)]
//! Independent extraction layout, position and search vectors.

use pagura::text::{LayoutOptions, SearchOptions, TextMatch, TextPage, WritingMode};
use pagura::{Document, OpenOptions, RenderOptions, Value};
// The expected vectors are written in PDF object syntax; the parser only
// reads the oracle and is not under test.
use pagura_core::syntax::{Origin, Parser};
use pagura_testkit::{Manifest, fixture, fixture_path};

fn array(value: &Value) -> &[Value] {
    value.as_array().unwrap()
}
fn integer(value: &Value) -> usize {
    usize::try_from(value.as_integer().unwrap()).unwrap()
}
fn string(value: &Value) -> String {
    String::from_utf8(value.as_string().unwrap().to_vec()).unwrap()
}
fn boolean(value: &Value) -> bool {
    if let Value::Boolean(b) = value {
        *b
    } else {
        panic!("boolean expected")
    }
}
fn expected(name: &str, extension: &str) -> Value {
    let bytes = fixture("text", &format!("{name}.{extension}"));
    Parser::new(&bytes, Origin::Buffer, pagura::Limits::default())
        .parse()
        .unwrap()
}
fn close(actual: f64, expected: &Value) {
    let number = expected.as_number().unwrap();
    assert!((actual - number).abs() < 1e-9, "{actual} != {number}");
}
fn cases() -> Vec<String> {
    Manifest::load("text")
        .cases()
        .map(|case| case.name().to_owned())
        .filter(|name| name.starts_with("extract-"))
        .collect()
}

#[test]
fn positioned_semantics_layout_and_search_match_independent_generator() {
    let names = cases();
    assert!(names.len() >= 20);
    for name in names {
        let document = Document::open(
            fixture("text", &format!("{name}.pdf")),
            &OpenOptions::default(),
        )
        .unwrap();
        let page = document.page(0).unwrap();
        let (list, text) = pagura::compile_page_with_text(&document, page)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(
            text.text().as_bytes(),
            fixture("text", &format!("{name}.text")),
            "{name}: reading order"
        );
        assert_eq!(
            text.content_text().as_bytes(),
            fixture("text", &format!("{name}.content")),
            "{name}: content order"
        );
        check_glyphs(&name, &text);
        check_metadata(&name, &text);
        let lines = expected(&name, "lines");
        assert_eq!(
            text.lines().len(),
            array(&lines).len(),
            "{name}: line count"
        );
        for (line, row) in text.lines().iter().zip(array(&lines)) {
            let row = array(row);
            assert_eq!(line.range, integer(&row[0])..integer(&row[1]), "{name}");
            assert_eq!(line.words, integer(&row[2])..integer(&row[3]));
            assert_eq!(line.paragraph_end, boolean(&row[4]));
        }
        let words = expected(&name, "words");
        assert_eq!(text.words().len(), array(&words).len(), "{name}");
        for (word, row) in text.words().iter().zip(array(&words)) {
            let row = array(row);
            assert_eq!(word.range, integer(&row[0])..integer(&row[1]), "{name}");
            assert_eq!(word.line, integer(&row[2]));
        }
        let queries = expected(&name, "queries");
        for row in array(&queries) {
            let row = array(row);
            let options = SearchOptions {
                case_insensitive: boolean(&row[1]),
                whole_words: boolean(&row[2]),
                join_hyphenated: boolean(&row[3]),
            };
            let matches = text.find(&string(&row[0]), options).unwrap();
            let ranges: Vec<_> = matches.iter().map(|m| m.range.clone()).collect();
            let expected: Vec<_> = array(&row[4])
                .iter()
                .map(|v| {
                    let r = array(v);
                    integer(&r[0])..integer(&r[1])
                })
                .collect();
            assert_eq!(ranges, expected, "{name}: {}", string(&row[0]));
            check_highlights(&name, &matches, array(&row[5]));
        }
        let hidden = fixture_path("text", &format!("{name}.hidden-text"));
        if hidden.exists() {
            assert_eq!(
                text.with_layout(LayoutOptions {
                    include_hidden: true,
                    ..Default::default()
                })
                .unwrap()
                .text()
                .as_bytes(),
                std::fs::read(hidden).unwrap()
            );
        }
        // Both paths must produce the exact same pixels; extraction has no
        // effect on clipping, Type 3 execution or optional-content painting.
        let normal = pagura::compile_page(&document, page).unwrap();
        let options = RenderOptions::default();
        assert_eq!(
            list.render(&options).unwrap().data(),
            normal.render(&options).unwrap().data(),
            "{name}: rendering changed"
        );
    }
}

fn check_glyphs(name: &str, text: &TextPage) {
    let rows = expected(name, "glyphs");
    assert_eq!(text.glyphs().len(), array(&rows).len(), "{name}");
    for (glyph, row) in text.glyphs().iter().zip(array(&rows)) {
        let row = array(row);
        assert_eq!(
            glyph.unicode.as_deref(),
            if row[0] == Value::Null {
                None
            } else {
                Some(std::str::from_utf8(row[0].as_string().unwrap()).unwrap())
            },
            "{name}"
        );
        for (point, values) in glyph.quad.0.iter().zip(row[1..9].as_chunks::<2>().0.iter()) {
            close(point.x, &values[0]);
            close(point.y, &values[1]);
        }
        close(glyph.size, &row[9]);
        assert_eq!(
            glyph.writing_mode == WritingMode::Vertical,
            boolean(&row[10])
        );
        assert_eq!(glyph.render_mode, u8::try_from(integer(&row[11])).unwrap());
        assert_eq!(glyph.hidden, boolean(&row[12]));
        let path = glyph
            .marked_content
            .iter()
            .map(|m| std::str::from_utf8(m.tag.as_bytes()).unwrap())
            .collect::<Vec<_>>()
            .join("/");
        assert_eq!(path, string(&row[13]));
        for (a, b) in [
            glyph.origin.x,
            glyph.origin.y,
            glyph.axis.x,
            glyph.axis.y,
            glyph.em,
            glyph.advance_end.x,
            glyph.advance_end.y,
        ]
        .into_iter()
        .zip(&row[14..])
        {
            close(a, b);
        }
    }
}

fn check_highlights(name: &str, matches: &[TextMatch], expected_indices: &[Value]) {
    let glyphs = expected(name, "glyphs");
    assert_eq!(matches.len(), expected_indices.len());
    for (found, indices) in matches.iter().zip(expected_indices) {
        assert_eq!(found.quads.len(), array(indices).len());
        for (quad, index) in found.quads.iter().zip(array(indices)) {
            let row = array(&array(&glyphs)[integer(index)]);
            for (point, values) in quad.0.iter().zip(row[1..9].as_chunks::<2>().0) {
                close(point.x, &values[0]);
                close(point.y, &values[1]);
            }
        }
    }
}

fn check_metadata(name: &str, text: &TextPage) {
    if !text.glyphs().is_empty() {
        assert_eq!(text.fonts().len(), 1);
        assert_eq!(text.fonts()[0].object, Some(pagura::ObjectId::new(5, 0)));
        assert!(text.glyphs().iter().all(|g| g.font == 0));
    }
    if fixture_path("text", &format!("{name}.marked")).exists() {
        let data = expected(name, "marked");
        assert_eq!(text.marked_content().len(), array(&data).len());
        for (span, row) in text.marked_content().iter().zip(array(&data)) {
            let row = array(row);
            assert_eq!(span.content.tag.as_bytes(), string(&row[0]).as_bytes());
            assert_eq!(span.content.mcid, row[1].as_integer());
            assert_eq!(span.content.actual_text.as_ref(), Some(&string(&row[2])));
            assert_eq!(span.content.alt.as_ref(), Some(&string(&row[3])));
            assert_eq!(span.glyphs, integer(&row[4])..integer(&row[5]));
            assert_eq!(span.hidden, boolean(&row[6]));
        }
    }
}
