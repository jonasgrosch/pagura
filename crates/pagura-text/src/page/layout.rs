//! Deterministic layout policy: .2-em gaps, .35-em baselines, 1.8-em paragraphs.
//! Unmarked lines cluster by inline anchors (1.5 em), then run top to bottom;
//! marked sequences keep content order. Orientation groups keep discovery order.
use super::{
    Arc, Error, LayoutOptions, Limits, Point, PositionedGlyph, PositionedText, Range, Result,
    TextPage, charge,
};
use std::collections::{BTreeMap, BTreeSet};
pub(super) mod copy;
pub(super) mod geometry;
use copy::{append, export};
use geometry::{content_lines, order};

pub(super) struct Atom {
    pub(super) chars: Vec<Option<char>>,
    pub(super) glyphs: Range<usize>,
    pub(super) geometry: Option<usize>,
    pub(super) marked: Option<usize>,
    pub(super) replacement: Option<Replacement>,
    pub(super) occurrence: Option<usize>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Replacement {
    Actual,
    Alt,
}
struct Line {
    atoms: Vec<usize>,
    geometry: Option<usize>,
    marked: Option<usize>,
}

pub(super) fn build(
    source: Arc<PositionedText>,
    options: LayoutOptions,
    limits: Limits,
    source_retained: Option<usize>,
) -> Result<TextPage> {
    validate(&source, options, limits)?;
    let source_retained = source_retained.unwrap_or_else(|| super::memory::source_bytes(&source));
    if source_retained > limits.document_bytes {
        return Err(Error::limit("positioned text retained budget exceeded"));
    }
    let atoms = atoms(&source, options, limits)?;
    let mut page = TextPage {
        positioned: source,
        content: Vec::new(),
        content_text: String::new(),
        reading: Vec::new(),
        text: String::new(),
        lines: Vec::new(),
        words: Vec::new(),
        limits,
        retained: 0,
        source_retained,
        options,
        logical: None,
        structure_fallback: None,
    };
    let mut alternate = false;
    for atom in &atoms {
        let current = atom.replacement == Some(Replacement::Alt);
        if alternate
            && current
            && !page.content_text.ends_with(char::is_whitespace)
            && atom
                .chars
                .first()
                .is_some_and(|c| c.is_some_and(|c| !c.is_whitespace()))
        {
            append(
                &mut page.content,
                &mut page.content_text,
                Some(' '),
                0..0,
                true,
            );
        }
        alternate = current;
        for &unicode in &atom.chars {
            append(
                &mut page.content,
                &mut page.content_text,
                unicode,
                atom.glyphs.clone(),
                false,
            );
        }
    }
    let lines = content_lines(&page.positioned, &atoms, options);
    let ordered = order(&page.positioned, lines);
    export(&mut page, &atoms, ordered, options)?;
    page.retained = super::memory::estimate(&page);
    if page.retained > limits.document_bytes {
        return Err(Error::limit("text page retained budget exceeded"));
    }
    Ok(page)
}

fn validate(source: &PositionedText, options: LayoutOptions, limits: Limits) -> Result<()> {
    let count = validate_records(source, options, limits)?;
    let mut work = count.saturating_mul(
        usize::try_from(count.bit_width())
            .unwrap_or(usize::MAX)
            .saturating_add(8),
    );
    charge(&mut work, 0, limits.operations)?;
    let mut bytes = count.saturating_mul(size_of::<PositionedGlyph>() + 256);
    let mut string_bytes = count.saturating_mul(2);
    charge(&mut bytes, 0, limits.document_bytes)?;
    for glyph in &source.glyphs {
        validate_glyph(glyph, source.fonts.len())?;
        charge(
            &mut work,
            glyph.unicode.as_ref().map_or(1, String::len),
            limits.operations,
        )?;
        charge(
            &mut string_bytes,
            glyph.unicode.as_ref().map_or(3, String::len),
            limits.string_bytes,
        )?;
        charge(
            &mut bytes,
            glyph
                .unicode
                .as_ref()
                .map_or(1, String::len)
                .saturating_mul(128),
            limits.document_bytes,
        )?;
        charge(
            &mut bytes,
            glyph.marked_content.len().saturating_mul(16),
            limits.document_bytes,
        )?;
        if glyph.marked_content.len() > limits.depth {
            return Err(Error::limit("text marked-content depth exceeded"));
        }
    }
    for span in &source.marked {
        if span.glyphs.start > span.glyphs.end || span.glyphs.end > source.glyphs.len() {
            return Err(Error::invalid_argument(
                "invalid marked-content glyph range",
            ));
        }
        if span.ancestors.len() > limits.depth {
            return Err(Error::limit("text marked-content depth exceeded"));
        }
        charge(
            &mut work,
            span.content
                .actual_text
                .as_ref()
                .or(span.content.alt.as_ref())
                .map_or(0, String::len),
            limits.operations,
        )?;
        charge(
            &mut string_bytes,
            span.content
                .actual_text
                .as_ref()
                .or(span.content.alt.as_ref())
                .map_or(0, String::len),
            limits.string_bytes,
        )?;
        charge(
            &mut bytes,
            span.content
                .actual_text
                .as_ref()
                .map_or(0, String::len)
                .saturating_mul(128),
            limits.document_bytes,
        )?;
        charge(
            &mut bytes,
            span.content
                .alt
                .as_ref()
                .map_or(0, String::len)
                .saturating_mul(128),
            limits.document_bytes,
        )?;
    }
    Ok(())
}

fn validate_records(
    source: &PositionedText,
    options: LayoutOptions,
    limits: Limits,
) -> Result<usize> {
    if [
        options.space_gap,
        options.baseline_tolerance,
        options.paragraph_gap,
    ]
    .into_iter()
    .any(|n| !n.is_finite() || n < 0.0)
    {
        return Err(Error::invalid_argument("invalid text layout thresholds"));
    }
    if source.glyphs.len() > limits.value_nodes
        || source.marked.len() > limits.value_nodes
        || source.objects.len() > limits.value_nodes
        || source.fonts.len() > limits.value_nodes
    {
        return Err(Error::limit("positioned text entry budget exceeded"));
    }
    let count = source
        .glyphs
        .len()
        .saturating_add(source.marked.len())
        .saturating_add(source.objects.len());
    for object in &source.objects {
        if object.glyphs.start > object.glyphs.end || object.glyphs.end > source.glyphs.len() {
            return Err(Error::invalid_argument(
                "invalid extraction object glyph range",
            ));
        }
        if object.marked_content.len() > limits.depth {
            return Err(Error::limit("text object marked-content depth exceeded"));
        }
    }
    Ok(count)
}

pub(super) fn atoms(
    source: &PositionedText,
    options: LayoutOptions,
    limits: Limits,
) -> Result<Vec<Atom>> {
    let mut replacements = BTreeMap::new();
    let mut selected = BTreeSet::new();
    for span in &source.marked {
        if span.ancestors.iter().any(|a| selected.contains(&a.id)) {
            continue;
        }
        if span.content.actual_text.is_some() || span.content.alt.is_some() {
            selected.insert(span.content.id);
            replacements
                .entry(span.glyphs.start)
                .or_insert_with(Vec::new)
                .push(span);
        }
    }
    let mut result = Vec::new();
    let mut index = 0;
    while index <= source.glyphs.len() {
        if let Some(spans) = replacements.get(&index) {
            for span in spans {
                if !span.hidden || options.include_hidden {
                    let text = span
                        .content
                        .actual_text
                        .as_ref()
                        .or(span.content.alt.as_ref());
                    result.push(Atom {
                        occurrence: Some(span.content.id),
                        chars: text.into_iter().flat_map(|s| s.chars()).map(Some).collect(),
                        glyphs: span.glyphs.clone(),
                        geometry: (span.glyphs.start < span.glyphs.end)
                            .then_some(span.glyphs.start),
                        replacement: Some(if span.content.actual_text.is_some() {
                            Replacement::Actual
                        } else {
                            Replacement::Alt
                        }),
                        marked: span
                            .ancestors
                            .first()
                            .map(|a| a.id)
                            .or(Some(span.content.id)),
                    });
                }
                index = index.max(span.glyphs.end);
            }
            if spans.iter().any(|s| !s.glyphs.is_empty()) {
                continue;
            }
        }
        let Some(glyph) = source.glyphs.get(index) else {
            break;
        };
        let end = if reversed(glyph) {
            let mut end = index + 1;
            while source
                .glyphs
                .get(end)
                .is_some_and(|g| g.show == glyph.show && reversed(g))
            {
                end += 1;
            }
            end
        } else {
            index + 1
        };
        for g in (index..end).map(|i| {
            if reversed(glyph) {
                end - 1 - (i - index)
            } else {
                i
            }
        }) {
            let record = &source.glyphs[g];
            if record.hidden && !options.include_hidden {
                continue;
            }
            let chars = record
                .unicode
                .as_ref()
                .map_or_else(|| vec![None], |s| s.chars().map(Some).collect());
            result.push(Atom {
                occurrence: None,
                chars,
                glyphs: g..g + 1,
                geometry: Some(g),
                marked: record.marked_content.first().map(|m| m.id),
                replacement: None,
            });
        }
        index = end;
        if result.len() > limits.value_nodes {
            return Err(Error::limit("text atom budget exceeded"));
        }
    }
    Ok(result)
}
fn reversed(glyph: &PositionedGlyph) -> bool {
    glyph
        .marked_content
        .iter()
        .any(|m| m.tag.as_bytes() == b"ReversedChars")
}

fn validate_glyph(glyph: &PositionedGlyph, fonts: usize) -> Result<()> {
    if glyph.font >= fonts
        || glyph.render_mode > 7
        || glyph.em < 0.0
        || ![glyph.size, glyph.em].into_iter().all(f64::is_finite)
        || !glyph
            .quad
            .0
            .into_iter()
            .chain([glyph.origin, glyph.axis, glyph.advance_end])
            .all(Point::is_finite)
    {
        return Err(Error::invalid_argument("invalid positioned glyph"));
    }
    Ok(())
}
