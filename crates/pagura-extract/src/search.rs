//! Search policy: Unicode uppercase mapping, whitespace folding, optional
//! discretionary line hyphens. No normalization or locale-dependent case fold.
use crate::{Quad, TextPage};
use pagura_core::Meter;
use pagura_core::{Error, Result};
use std::collections::{HashSet, VecDeque};
use std::ops::Range;

/// Search policy. Case mapping uses Rust's Unicode `char::to_uppercase`.
/// Hosts build options from [`Default`] with struct update syntax, so the
/// struct is exhaustive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchOptions {
    /// Match without case distinctions, including uppercase expansions.
    pub case_insensitive: bool,
    /// Require alphanumeric/underscore boundaries (not UAX #29 segmentation).
    pub whole_words: bool,
    /// Join a hyphen followed by a single line break between letters.
    pub join_hyphenated: bool,
}
impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            case_insensitive: true,
            whole_words: false,
            join_hyphenated: true,
        }
    }
}
/// One match in reading-order UTF-8 bytes, with source selection cells.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct TextMatch {
    /// Byte range in [`TextPage::text`], including intervening line hyphens.
    pub range: Range<usize>,
    /// Each contributing source glyph's quad, once, in reading order.
    pub quads: Vec<Quad>,
}
/// Storage per query byte: its pattern characters, prefix table and
/// matching window.
const QUERY_BYTES: usize = 128;
/// Work per query byte to case-map it and build the prefix table.
const QUERY_WORK: usize = 4;
/// Amortized matching work per searched unit.
const UNIT_WORK: usize = 4;
/// Bookkeeping per highlighted glyph: its set entry and vector growth.
const QUAD_ENTRY_BYTES: usize = 64;

#[derive(Clone, Copy)]
struct Unit {
    value: char,
    start: usize,
    end: usize,
    first: bool,
    last: bool,
}
pub(super) fn word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl TextPage {
    /// Searches copy text, folding whitespace across line breaks. Unknown
    /// Unicode is a barrier. A partial Unicode case expansion never matches.
    /// Replacement text highlights all glyphs of its associated occurrence.
    ///
    /// # Errors
    /// Returns a limit error for excessive query, work or result storage.
    pub fn find(&self, query: &str, options: SearchOptions) -> Result<Vec<TextMatch>> {
        if query.len() > self.limits.string_bytes {
            return Err(Error::limit("search query byte budget exceeded"));
        }
        let mut bytes = Meter::new(
            self.limits.document_bytes,
            "text search storage budget exceeded",
        );
        bytes.charge_product(query.len(), QUERY_BYTES)?;
        let mut work = Meter::new(self.limits.text_work, "text search work budget exceeded");
        work.charge_product(query.len(), QUERY_WORK)?;
        let pattern = pattern(query, options);
        if pattern.is_empty() {
            return Ok(Vec::new());
        }
        let mut prefix = vec![0; pattern.len()];
        for i in 1..pattern.len() {
            let mut j = prefix[i - 1];
            while j > 0 && pattern[i] != pattern[j] {
                j = prefix[j - 1];
            }
            if pattern[i] == pattern[j] {
                j += 1;
            }
            prefix[i] = j;
        }
        let mut state = Search {
            page: self,
            options,
            pattern,
            prefix,
            window: VecDeque::new(),
            matched: 0,
            result: Vec::new(),
            bytes,
            work,
        };
        state.run()?;
        Ok(state.result)
    }
}
fn pattern(query: &str, options: SearchOptions) -> Vec<char> {
    let mut result = Vec::new();
    for c in query.chars() {
        if c.is_whitespace() {
            if result.last() != Some(&' ') {
                result.push(' ');
            }
        } else if options.case_insensitive {
            result.extend(c.to_uppercase());
        } else {
            result.push(c);
        }
    }
    result
}
struct Search<'a> {
    page: &'a TextPage,
    options: SearchOptions,
    pattern: Vec<char>,
    prefix: Vec<usize>,
    window: VecDeque<Unit>,
    matched: usize,
    result: Vec<TextMatch>,
    bytes: Meter,
    work: Meter,
}
impl Search<'_> {
    fn run(&mut self) -> Result<()> {
        let chars = &self.page.reading;
        self.work.charge(chars.len())?;
        let mut index = 0;
        while index < chars.len() {
            let Some(c) = chars[index].unicode else {
                self.matched = 0;
                self.window.clear();
                index += 1;
                continue;
            };
            if self.options.join_hyphenated
                && matches!(c, '-' | '\u{2010}' | '\u{ad}')
                && index > 0
                && chars[index - 1].unicode.is_some_and(char::is_alphabetic)
            {
                let mut end = index + 1;
                let mut breaks = 0;
                while end < chars.len() && chars[end].unicode.is_some_and(char::is_whitespace) {
                    breaks += usize::from(chars[end].unicode == Some('\n'));
                    end += 1;
                }
                if breaks == 1
                    && chars
                        .get(end)
                        .is_some_and(|c| c.unicode.is_some_and(char::is_alphabetic))
                {
                    index = end;
                    continue;
                }
            }
            let mut end = index + 1;
            if c.is_whitespace() {
                while end < chars.len() && chars[end].unicode.is_some_and(char::is_whitespace) {
                    end += 1;
                }
                self.push(Unit {
                    value: ' ',
                    start: index,
                    end,
                    first: true,
                    last: true,
                })?;
            } else if self.options.case_insensitive {
                let mut mapped = c.to_uppercase().peekable();
                let mut first = true;
                while let Some(value) = mapped.next() {
                    self.push(Unit {
                        value,
                        start: index,
                        end,
                        first,
                        last: mapped.peek().is_none(),
                    })?;
                    first = false;
                }
            } else {
                self.push(Unit {
                    value: c,
                    start: index,
                    end,
                    first: true,
                    last: true,
                })?;
            }
            index = end;
        }
        Ok(())
    }
    fn push(&mut self, unit: Unit) -> Result<()> {
        self.work.charge(UNIT_WORK)?;
        while self.matched > 0 && unit.value != self.pattern[self.matched] {
            self.matched = self.prefix[self.matched - 1];
        }
        if unit.value == self.pattern[self.matched] {
            self.matched += 1;
        }
        self.window.push_back(unit);
        if self.window.len() > self.pattern.len() {
            self.window.pop_front();
        }
        if self.matched == self.pattern.len() {
            if let Some(first) = self.window.front().copied()
                && first.first
                && unit.last
                && self.boundaries(first.start, unit.end)
            {
                self.record(first.start..unit.end)?;
            }
            self.matched = self.prefix[self.matched - 1];
        }
        Ok(())
    }
    fn boundaries(&self, start: usize, end: usize) -> bool {
        !self.options.whole_words
            || (start
                .checked_sub(1)
                .and_then(|i| self.page.reading[i].unicode)
                .is_none_or(|c| !word(c))
                && self
                    .page
                    .reading
                    .get(end)
                    .and_then(|c| c.unicode)
                    .is_none_or(|c| !word(c)))
    }
    fn record(&mut self, range: Range<usize>) -> Result<()> {
        self.bytes.charge_items::<TextMatch>(1)?;
        let mut seen = HashSet::new();
        let mut quads = Vec::new();
        for character in &self.page.reading[range.clone()] {
            self.work.charge(1)?;
            for glyph in character.glyphs.clone() {
                self.work.charge(1)?;
                if !seen.contains(&glyph) {
                    self.bytes.charge(size_of::<Quad>() + QUAD_ENTRY_BYTES)?;
                    seen.insert(glyph);
                    quads.push(self.page.positioned.glyphs[glyph].quad);
                }
            }
        }
        let start = self.page.reading[range.start].range.start;
        let end = self.page.reading[range.end - 1].range.end;
        self.result.push(TextMatch {
            range: start..end,
            quads,
        });
        Ok(())
    }
}
