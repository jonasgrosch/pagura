//! `extraction`: a content stream compiles into a display list and its
//! positioned text in one interpretation (ADR 0014), or fails with a
//! structured error, in both recovery modes. Characters, lines, words and
//! glyph records stay inside the text they describe, every quad is finite,
//! and search finds only ranges of the text that fold to the query.
//!
//! The input is a content stream on the fixed page of [`super::content`].
//! Its Type 3 font `/F` maps code 97 to `a`; marked content with
//! `/ActualText` or `/Alt` supplies any other text. When the input's last
//! line starts with `%`, the rest of that line is a search query; a slice of
//! the extracted text is searched too, each with every option combination.

use pagura::{
    DisplayList, Document, OpenOptions, Recovery, RenderOptions, SearchOptions, TextPage,
};
use pagura_extract::Character;

/// The most characters of the extracted text searched for.
const QUERY_CHARS: usize = 8;

/// Compiles the page with and without extraction and checks the text.
///
/// # Panics
///
/// When the input violates a checked property: that is a finding.
pub fn run(data: &[u8]) {
    let comment = comment_query(data);
    for recovery in [Recovery::Strict, Recovery::Repair] {
        let options = OpenOptions {
            limits: super::limits(),
            recovery,
            ..OpenOptions::default()
        };
        let Some(document) = super::checked(Document::open(super::content::wrap(data), &options))
        else {
            continue;
        };
        let Some(page) = super::checked(document.page(0)) else {
            continue;
        };
        // The page's only fonts are Type 3 fonts, which need no provider, so
        // the facade's bundled data and none select the same glyphs.
        let compiled = pagura::compile_page_with_text(&document, page, None);
        let plain = pagura::compile_page(&document, page);
        let text = pagura::text_page(&document, page);
        match &compiled {
            Ok((list, extracted)) => {
                let plain = plain.as_ref();
                assert!(plain.is_ok(), "only the extracting compilation succeeded");
                if let Ok(plain) = plain {
                    assert_same_list(list, plain);
                }
                assert!(
                    text.as_ref().is_ok_and(|text| same_text(text, extracted)),
                    "text_page differs from the extracting compilation"
                );
                assert_text(extracted);
                for query in comment.iter().cloned().chain(text_query(extracted)) {
                    search(extracted, &query);
                }
            }
            Err(error) => {
                assert!(
                    text.as_ref().err() == Some(error),
                    "text_page fails differently"
                );
                if let Err(plain) = &plain {
                    super::assert_structured(plain);
                }
            }
        }
        super::checked(compiled);
        super::assert_diagnostics(&document.diagnostics(), recovery);
    }
}

/// Extraction leaves the display list unchanged: the same marks, rendering
/// the same pixels.
fn assert_same_list(list: &DisplayList, plain: &DisplayList) {
    assert_eq!(list.len(), plain.len(), "extraction changed the mark count");
    let render = RenderOptions::default();
    let pixels = |list: &DisplayList| list.render(&render).map(|p| p.data().to_vec());
    let pixels = (pixels(list), pixels(plain));
    assert!(pixels.0 == pixels.1, "extraction changed the rendered page");
    super::checked(pixels.0);
}

fn same_text(a: &TextPage, b: &TextPage) -> bool {
    a.text() == b.text()
        && a.content_text() == b.content_text()
        && a.characters() == b.characters()
        && a.reading_characters() == b.reading_characters()
        && a.lines() == b.lines()
        && a.words() == b.words()
        && a.fonts() == b.fonts()
        && a.glyphs().len() == b.glyphs().len()
        && a.glyphs()
            .iter()
            .zip(b.glyphs())
            .all(|(a, b)| a.quad == b.quad)
}

/// Characters partition their string, one Unicode scalar each, and name
/// source glyphs that exist; lines and words lie inside the text; glyph
/// geometry is finite.
fn assert_text(page: &TextPage) {
    let glyphs = page.glyphs();
    for (name, characters, text) in [
        ("content", page.characters(), page.content_text()),
        ("reading", page.reading_characters(), page.text()),
    ] {
        assert_partition(name, characters, text, glyphs.len());
    }
    let text = page.text();
    let limits = super::limits();
    assert!(
        text.len() <= limits.string_bytes,
        "{} bytes of text",
        text.len()
    );
    for (index, line) in page.lines().iter().enumerate() {
        let content = text.get(line.range.clone());
        assert!(
            content.is_some_and(|s| !s.contains('\n')),
            "line {index} {:?} in {} bytes",
            line.range,
            text.len()
        );
        assert!(
            line.words.start <= line.words.end && line.words.end <= page.words().len(),
            "line {index} words {:?}",
            line.words
        );
    }
    for (index, word) in page.words().iter().enumerate() {
        let line = page.lines().get(word.line);
        assert!(
            line.is_some_and(|l| l.range.start <= word.range.start
                && word.range.end <= l.range.end
                && l.words.contains(&index))
                && text.get(word.range.clone()).is_some_and(|s| !s.is_empty()),
            "word {index} {:?} on line {}",
            word.range,
            word.line
        );
    }
    for (index, glyph) in glyphs.iter().enumerate() {
        assert!(
            glyph.quad.0.iter().all(|p| p.is_finite())
                && [glyph.origin, glyph.axis, glyph.advance_end]
                    .iter()
                    .all(|p| p.is_finite())
                && glyph.size.is_finite()
                && glyph.em.is_finite()
                && glyph.em >= 0.0
                && glyph.font < page.fonts().len()
                && u8::from(glyph.render_mode) <= 7,
            "glyph {index}: {glyph:?}"
        );
    }
    for span in page.marked_content() {
        assert!(
            span.glyphs.start <= span.glyphs.end && span.glyphs.end <= glyphs.len(),
            "marked content over glyphs {:?}",
            span.glyphs
        );
    }
}

/// One character after another from the start to the end of `text`, each
/// the UTF-8 of its scalar (U+FFFD when unknown); an inserted separator has
/// no source glyphs.
fn assert_partition(name: &str, characters: &[Character], text: &str, glyphs: usize) {
    let mut end = 0;
    for (index, character) in characters.iter().enumerate() {
        let scalar = character.unicode.unwrap_or('\u{fffd}');
        let mut buffer = [0; 4];
        assert!(
            character.range.start == end
                && text.get(character.range.clone()) == Some(scalar.encode_utf8(&mut buffer)),
            "{name} character {index} {character:?} after byte {end} of {} bytes",
            text.len()
        );
        assert!(
            character.glyphs.start <= character.glyphs.end
                && character.glyphs.end <= glyphs
                && (!character.inserted || character.glyphs.is_empty()),
            "{name} character {index} {character:?} of {glyphs} glyphs"
        );
        end = character.range.end;
    }
    assert_eq!(end, text.len(), "{name} characters end before the text");
}

/// Every match is a nonempty range of the text with finite quads. Without
/// joined hyphens, its text folds to the query: whitespace runs as one
/// space, and case through Unicode uppercase mapping unless case matters.
fn search(page: &TextPage, query: &str) {
    for bits in 0..8_u8 {
        let options = SearchOptions {
            case_insensitive: bits & 1 != 0,
            whole_words: bits & 2 != 0,
            join_hyphenated: bits & 4 != 0,
        };
        let Some(matches) = super::checked(page.find(query, options)) else {
            continue;
        };
        for found in &matches {
            let text = page.text().get(found.range.clone());
            assert!(
                text.is_some_and(|s| !s.is_empty()),
                "match {:?} in {} bytes",
                found.range,
                page.text().len()
            );
            assert!(
                found
                    .quads
                    .iter()
                    .all(|q| q.0.iter().all(|p| p.is_finite())),
                "match quads {:?}",
                found.quads
            );
            if let Some(text) = text
                && !options.join_hyphenated
            {
                assert_eq!(
                    fold(text, options.case_insensitive),
                    fold(query, options.case_insensitive),
                    "{options:?} match for {query:?}"
                );
            }
        }
    }
}

/// The search's view of a string: whitespace runs fold to one space and,
/// without case distinctions, characters map to uppercase (ADR 0014).
pub(crate) fn fold(text: &str, case_insensitive: bool) -> Vec<char> {
    let mut folded = Vec::new();
    for c in text.chars() {
        if c.is_whitespace() {
            if folded.last() != Some(&' ') {
                folded.push(' ');
            }
        } else if case_insensitive {
            folded.extend(c.to_uppercase());
        } else {
            folded.push(c);
        }
    }
    folded
}

/// The input's last line, when it is a comment, without its `%`.
fn comment_query(data: &[u8]) -> Option<String> {
    let data = data.strip_suffix(b"\n").unwrap_or(data);
    let line = data.rsplit(|&b| b == b'\n' || b == b'\r').next()?;
    let query = line.strip_prefix(b"%")?;
    Some(String::from_utf8_lossy(query).into_owned())
}

/// Up to [`QUERY_CHARS`] characters from a third of the way into the text.
fn text_query(page: &TextPage) -> Option<String> {
    let text = page.text();
    let skip = text.chars().count() / 3;
    let query: String = text.chars().skip(skip).take(QUERY_CHARS).collect();
    (!query.is_empty()).then_some(query)
}
