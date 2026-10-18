//! Copy text, semantic separators and byte-indexed word/line boundaries.
use super::geometry::{FRAGMENT_EMS, PARALLEL, coordinates, direction, dot, em, gap, same_line};
use super::{Atom, Line, Replacement, reversed};
use crate::{Character, LayoutOptions, PositionedText, TextLine, TextPage, TextWord, search};
use pagura_core::{Error, Result};
use std::ops::Range;
/// Whether layout decoded a character from glyphs or inserted it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Origin {
    Decoded,
    Inserted,
}

pub(crate) fn append(
    chars: &mut Vec<Character>,
    text: &mut String,
    unicode: Option<char>,
    glyphs: Range<usize>,
    origin: Origin,
) {
    let start = text.len();
    text.push(unicode.unwrap_or('\u{fffd}'));
    chars.push(Character {
        unicode,
        glyphs,
        inserted: origin == Origin::Inserted,
        range: start..text.len(),
    });
}

/// Whether a separator between `text` and `atom` would be redundant:
/// either side is already whitespace.
pub(crate) fn borders_whitespace(text: &str, atom: &Atom) -> bool {
    text.ends_with(char::is_whitespace)
        || atom
            .chars
            .first()
            .is_some_and(|c| c.is_some_and(char::is_whitespace))
}
pub(super) fn export(
    page: &mut TextPage,
    atoms: &[Atom],
    lines: Vec<Line>,
    options: LayoutOptions,
) -> Result<()> {
    let merged = merge_lines(&page.positioned, atoms, lines, options);
    let mut previous_line = None;
    for (index, line) in merged.into_iter().enumerate() {
        if index > 0 {
            let paragraph =
                previous_line
                    .zip(line.geometry)
                    .is_some_and(|(a, b): (usize, usize)| {
                        let first = &page.positioned.glyphs[a];
                        let next = &page.positioned.glyphs[b];
                        let axis = direction(first);
                        let (au, av) = coordinates(first.origin, axis);
                        let (bu, bv) = coordinates(next.origin, axis);
                        let em = em(first, next);
                        (av - bv).abs() > options.paragraph_gap * em
                            || (au - bu).abs() > FRAGMENT_EMS * em
                            || dot(axis, direction(next)) < PARALLEL
                    });
            append(
                &mut page.reading,
                &mut page.text,
                Some('\n'),
                0..0,
                Origin::Inserted,
            );
            if paragraph {
                append(
                    &mut page.reading,
                    &mut page.text,
                    Some('\n'),
                    0..0,
                    Origin::Inserted,
                );
            }
        }
        let mut previous: Option<usize> = None;
        let mut previous_alt = false;
        let mut previous_replacement = None;
        let mut previous_atom = false;
        for index in line.atoms {
            let atom = &atoms[index];
            let alt = atom.replacement == Some(Replacement::Alt);
            let whitespace = borders_whitespace(&page.text, atom);
            let semantic_space = previous_atom && (alt || previous_alt);
            let geometric_space = previous.zip(atom.geometry).is_some_and(|(a, b)| {
                let a = &page.positioned.glyphs[a];
                let b = &page.positioned.glyphs[b];
                let reversed_pair = reversed(a)
                    && reversed(b)
                    && a.marked_content.first().map(|m| m.id)
                        == b.marked_content.first().map(|m| m.id);
                let replacement_pair = previous_replacement == Some(Replacement::Actual)
                    && atom.replacement == Some(Replacement::Actual);
                !reversed_pair && !replacement_pair && gap(a, b) > options.space_gap * em(a, b)
            });
            if !whitespace && (semantic_space || geometric_space) {
                append(
                    &mut page.reading,
                    &mut page.text,
                    Some(' '),
                    0..0,
                    Origin::Inserted,
                );
            }
            for &unicode in &atom.chars {
                append(
                    &mut page.reading,
                    &mut page.text,
                    unicode,
                    atom.glyphs.clone(),
                    Origin::Decoded,
                );
            }
            previous = atom
                .glyphs
                .end
                .checked_sub(1)
                .filter(|i| !atom.glyphs.is_empty() && *i < page.positioned.glyphs.len())
                .or(atom.geometry);
            previous_alt = alt;
            previous_replacement = atom.replacement;
            previous_atom = true;
        }
        previous_line = line.geometry;
    }
    if page.text.len() > page.limits.string_bytes {
        return Err(Error::limit("extracted text byte budget exceeded"));
    }
    boundaries(page);
    Ok(())
}

fn words(text: &str, range: Range<usize>, line: usize, output: &mut Vec<TextWord>) {
    let mut start = None;
    for (offset, c) in text[range.clone()].char_indices() {
        if search::word(c) {
            start.get_or_insert(range.start + offset);
        } else if let Some(first) = start.take() {
            output.push(TextWord {
                range: first..range.start + offset,
                line,
            });
        }
    }
    if let Some(first) = start {
        output.push(TextWord {
            range: first..range.end,
            line,
        });
    }
}

fn merge_lines(
    source: &PositionedText,
    atoms: &[Atom],
    lines: Vec<Line>,
    options: LayoutOptions,
) -> Vec<Line> {
    let mut merged: Vec<Line> = Vec::new();
    for line in lines {
        let last = merged
            .last()
            .and_then(|l| l.atoms.last())
            .and_then(|i| atoms[*i].geometry);
        let replacements = merged
            .last()
            .and_then(|l| l.atoms.last())
            .zip(line.atoms.first())
            .is_some_and(|(a, b)| {
                atoms[*a].replacement == Some(Replacement::Actual)
                    && atoms[*b].replacement == Some(Replacement::Actual)
            });
        let unpositioned = !merged.is_empty() && (last.is_none() || line.geometry.is_none());
        let join = replacements
            || unpositioned
            || last.zip(line.geometry).is_some_and(|(a, b)| {
                let a = &source.glyphs[a];
                let b = &source.glyphs[b];
                same_line(a, b, options) && gap(a, b).abs() <= FRAGMENT_EMS * em(a, b)
            });
        if join {
            if let Some(previous) = merged.last_mut() {
                previous.atoms.extend(line.atoms);
            }
        } else {
            merged.push(line);
        }
    }
    merged
}

pub(crate) fn boundaries(page: &mut TextPage) {
    let mut start = 0;
    for text in page.text.split('\n') {
        let end = start + text.len();
        if !text.is_empty() {
            let word_start = page.words.len();
            words(&page.text, start..end, page.lines.len(), &mut page.words);
            page.lines.push(TextLine {
                range: start..end,
                words: word_start..page.words.len(),
                paragraph_end: page.text[end..].starts_with("\n\n"),
            });
        }
        start = end + 1;
    }
}
