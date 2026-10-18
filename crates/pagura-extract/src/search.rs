//! Search policy: Unicode uppercase mapping, whitespace folding, optional
//! discretionary line hyphens. No normalization or locale-dependent case fold.
use super::{Error, Quad, Range, Result, TextPage, charge};
use std::collections::{HashSet, VecDeque};

/// Search policy. Case mapping uses Rust's Unicode `char::to_uppercase`.
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
pub struct TextMatch {
    /// Byte range in [`TextPage::text`], including intervening line hyphens.
    pub range: Range<usize>,
    /// Each contributing source glyph's quad, once, in reading order.
    pub quads: Vec<Quad>,
}
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
        let mut bytes = query.len().saturating_mul(128);
        charge(&mut bytes, 0, self.limits.document_bytes)?;
        let mut work = query.len().saturating_mul(4);
        charge(&mut work, 0, self.limits.operations)?;
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
    bytes: usize,
    work: usize,
}
impl Search<'_> {
    fn run(&mut self) -> Result<()> {
        let chars = &self.page.reading;
        charge(&mut self.work, chars.len(), self.page.limits.operations)?;
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
        charge(&mut self.work, 4, self.page.limits.operations)?;
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
        charge(
            &mut self.bytes,
            size_of::<TextMatch>(),
            self.page.limits.document_bytes,
        )?;
        let mut seen = HashSet::new();
        let mut quads = Vec::new();
        for character in &self.page.reading[range.clone()] {
            charge(&mut self.work, 1, self.page.limits.operations)?;
            for glyph in character.glyphs.clone() {
                charge(&mut self.work, 1, self.page.limits.operations)?;
                if !seen.contains(&glyph) {
                    charge(
                        &mut self.bytes,
                        size_of::<Quad>() + 64,
                        self.page.limits.document_bytes,
                    )?;
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
