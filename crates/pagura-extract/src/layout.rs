//! Deterministic layout policy: .2-em gaps, .35-em baselines, 1.8-em paragraphs.
//! Unmarked lines cluster by inline anchors (1.5 em), then run top to bottom;
//! marked sequences keep content order. Orientation groups keep discovery order.
use crate::{LayoutOptions, PositionedGlyph, PositionedText, TextPage};
use pagura_core::{Error, Limits, Meter, Result};
use pagura_raster::Point;
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
    sync::Arc,
};
pub(super) mod copy;
pub(super) mod geometry;
use copy::{Origin, append, export};
use geometry::{content_lines, order};

pub(super) struct Atom {
    pub(super) chars: AtomText,
    pub(super) glyphs: Range<usize>,
    pub(super) geometry: Option<usize>,
    pub(super) marked: Option<usize>,
    pub(super) replacement: Option<Replacement>,
    pub(super) occurrence: Option<usize>,
}
/// The characters of an atom. Almost every atom is one glyph with one
/// character, stored inline instead of in a vector per glyph.
pub(super) enum AtomText {
    One(Option<char>),
    Many(Box<[Option<char>]>),
}

impl std::ops::Deref for AtomText {
    type Target = [Option<char>];

    fn deref(&self) -> &[Option<char>] {
        match self {
            Self::One(unicode) => std::slice::from_ref(unicode),
            Self::Many(chars) => chars,
        }
    }
}

impl<'a> IntoIterator for &'a AtomText {
    type Item = &'a Option<char>;
    type IntoIter = std::slice::Iter<'a, Option<char>>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl FromIterator<Option<char>> for AtomText {
    fn from_iter<I: IntoIterator<Item = Option<char>>>(iter: I) -> Self {
        let mut iter = iter.into_iter();
        match (iter.next(), iter.next()) {
            (Some(unicode), None) => Self::One(unicode),
            (None, _) => Self::Many(Box::default()),
            (Some(first), Some(second)) => {
                Self::Many([first, second].into_iter().chain(iter).collect())
            }
        }
    }
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
                Origin::Inserted,
            );
        }
        alternate = current;
        for &unicode in &atom.chars {
            append(
                &mut page.content,
                &mut page.content_text,
                unicode,
                atom.glyphs.clone(),
                Origin::Decoded,
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

/// Ordering `n` records takes about `n log n` comparisons; this is the
/// constant work per record for grouping and export on top of that.
const ORDER_WORK_PER_RECORD: usize = 8;
/// Storage a record can need besides itself: its line, word and character
/// entries in the content and reading views.
const RECORD_OUTPUT_BYTES: usize = 256;
/// Exported text a record can add as a separator: a space or line feeds.
const SEPARATOR_TEXT_BYTES: usize = 2;
/// Storage of one exported character: `Character` entries in the content
/// and reading views, its UTF-8 text in both strings and its search units.
const CHARACTER_OUTPUT_BYTES: usize = 128;
/// Storage of one entry of a marked-content path.
const MARK_PATH_BYTES: usize = 16;
/// Unknown Unicode exports U+FFFD.
const REPLACEMENT_TEXT_BYTES: usize = '\u{fffd}'.len_utf8();

/// Checks the records and charges an upper estimate of the layout's work,
/// storage and text before any of it is built.
fn validate(source: &PositionedText, options: LayoutOptions, limits: Limits) -> Result<()> {
    let count = validate_records(source, options, limits)?;
    let mut work = Meter::new(limits.text_work, "text layout work budget exceeded");
    let mut bytes = Meter::new(limits.document_bytes, "text page retained budget exceeded");
    let mut text = Meter::new(limits.string_bytes, "extracted text byte budget exceeded");
    let order = usize::try_from(count.bit_width())
        .unwrap_or(usize::MAX)
        .saturating_add(ORDER_WORK_PER_RECORD);
    work.charge_product(count, order)?;
    bytes.charge_product(count, size_of::<PositionedGlyph>() + RECORD_OUTPUT_BYTES)?;
    text.charge_product(count, SEPARATOR_TEXT_BYTES)?;
    for glyph in &source.glyphs {
        validate_glyph(glyph, source.fonts.len())?;
        let len = glyph.unicode.as_deref().map(str::len);
        work.charge(len.unwrap_or(1))?;
        text.charge(len.unwrap_or(REPLACEMENT_TEXT_BYTES))?;
        bytes.charge_product(len.unwrap_or(1), CHARACTER_OUTPUT_BYTES)?;
        bytes.charge_product(glyph.marked_content.len(), MARK_PATH_BYTES)?;
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
        let content = &span.content;
        let replacement = content.replacement().map_or(0, str::len);
        work.charge(replacement)?;
        text.charge(replacement)?;
        for value in [&content.actual_text, &content.alt] {
            bytes.charge_product(
                value.as_ref().map_or(0, String::len),
                CHARACTER_OUTPUT_BYTES,
            )?;
        }
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
    if source.glyphs.len() > limits.text_items
        || source.marked.len() > limits.text_items
        || source.objects.len() > limits.text_items
        || source.fonts.len() > limits.text_items
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
                    let text = span.content.replacement();
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
                .as_deref()
                .map_or(AtomText::One(None), |s| s.chars().map(Some).collect());
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
        if result.len() > limits.text_items {
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
