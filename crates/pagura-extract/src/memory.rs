//! Capacity accounting, with each reachable Arc allocation counted once.
use super::{LogicalText, MarkedContent, PositionedText, TextPage};
use pagura_core::{Name, Value};
use std::{collections::BTreeSet, sync::Arc};

pub(super) fn name_bytes(name: &Name) -> usize {
    // Delegate the inline/heap representation to core rather than duplicating
    // Name's private small-string threshold. This runs only at construction.
    Value::Name(name.clone())
        .retained_bytes()
        .saturating_sub(size_of::<Value>())
}
fn vector<T>(value: &Vec<T>) -> usize {
    value.capacity().saturating_mul(size_of::<T>())
}
struct Shared {
    paths: BTreeSet<usize>,
    marks: BTreeSet<usize>,
}
impl Shared {
    fn mark(&mut self, mark: &Arc<MarkedContent>) -> usize {
        if !self.marks.insert(Arc::as_ptr(mark).addr()) {
            return 0;
        }
        size_of::<MarkedContent>()
            .saturating_add(2 * size_of::<usize>())
            .saturating_add(name_bytes(&mark.tag))
            .saturating_add(mark.actual_text.as_ref().map_or(0, String::capacity))
            .saturating_add(mark.alt.as_ref().map_or(0, String::capacity))
    }
    fn path(&mut self, path: &Arc<[Arc<MarkedContent>]>) -> usize {
        if !self.paths.insert(Arc::as_ptr(path).cast::<()>().addr()) {
            return 0;
        }
        let mut bytes = path
            .len()
            .saturating_mul(size_of::<Arc<MarkedContent>>())
            .saturating_add(2 * size_of::<usize>());
        for mark in path.iter() {
            bytes = bytes.saturating_add(self.mark(mark));
        }
        bytes
    }
}
pub(super) fn source_bytes(source: &PositionedText) -> usize {
    let mut bytes = size_of_val(source).saturating_add(2 * size_of::<usize>());
    for part in [
        vector(&source.fonts),
        vector(&source.glyphs),
        vector(&source.marked),
        vector(&source.objects),
    ] {
        bytes = bytes.saturating_add(part);
    }
    let mut shared = Shared {
        paths: BTreeSet::new(),
        marks: BTreeSet::new(),
    };
    for font in &source.fonts {
        bytes = bytes.saturating_add(font.base_name.as_ref().map_or(0, name_bytes));
    }
    for glyph in &source.glyphs {
        bytes = bytes
            .saturating_add(glyph.unicode.as_ref().map_or(0, String::capacity))
            .saturating_add(shared.path(&glyph.marked_content));
    }
    for object in &source.objects {
        bytes = bytes.saturating_add(shared.path(&object.marked_content));
    }
    for span in &source.marked {
        bytes = bytes
            .saturating_add(shared.mark(&span.content))
            .saturating_add(shared.path(&span.ancestors));
    }
    bytes
}
pub(super) fn estimate(page: &TextPage) -> usize {
    let mut bytes = page.source_retained;
    for part in [
        vector(&page.content),
        vector(&page.reading),
        vector(&page.lines),
        vector(&page.words),
        page.content_text.capacity(),
        page.text.capacity(),
    ] {
        bytes = bytes.saturating_add(part);
    }
    if let Some(logical) = &page.logical {
        bytes = bytes.saturating_add(logical.retained);
    }
    bytes
}
// Includes the Arc allocation and its complete owned payload. Independently
// cached page views each charge this shared logical allocation in full, just
// as they charge their positioned source. Reference counts do not affect it.
pub(super) fn logical_bytes(logical: &LogicalText) -> usize {
    let mut bytes = size_of_val(logical)
        .saturating_add(2 * size_of::<usize>())
        .saturating_add(logical.text.capacity())
        .saturating_add(vector(&logical.characters))
        .saturating_add(vector(&logical.structure));
    for element in &logical.structure {
        bytes = bytes
            .saturating_add(name_bytes(&element.role))
            .saturating_add(element.standard_type.as_ref().map_or(0, name_bytes));
    }
    bytes
}
impl Clone for TextPage {
    fn clone(&self) -> Self {
        // Vec/String clones need not retain the original spare capacity.
        let mut result = Self {
            positioned: Arc::clone(&self.positioned),
            content: self.content.clone(),
            content_text: self.content_text.clone(),
            reading: self.reading.clone(),
            text: self.text.clone(),
            lines: self.lines.clone(),
            words: self.words.clone(),
            limits: self.limits,
            options: self.options,
            retained: 0,
            source_retained: self.source_retained,
            logical: self.logical.clone(),
            structure_fallback: self.structure_fallback.clone(),
        };
        result.retained = estimate(&result);
        result
    }
}
