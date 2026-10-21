//! The text device: positioned glyphs, marked content and object
//! occurrences for `pagura-extract` (ADR 0014, ADR 0025).

use crate::device::{Device, FontInfo, FontKey, GlyphRun, Marked, Needs, PageInfo};
use pagura_core::{Error, Limits, Meter, ObjectId, Result};
use pagura_extract::{
    MarkedContent, MarkedSpan, PositionedGlyph, PositionedText, TextFont, TextObject,
};
use std::collections::HashMap;
use std::sync::Arc;

/// Collects the positioned text of a page. It needs no paint, images or
/// outlines, so extraction decodes no image, parses no shading and converts
/// no colour.
///
/// Fonts are listed in selection order, glyphs in content order with the
/// marked-content path they occur in and the number of the string operand
/// that showed them, and form, image and annotation occurrences with the
/// glyphs they hold. Items are bounded by `text_items` and their storage by
/// `document_bytes`.
#[derive(Debug)]
pub struct TextDevice {
    limits: Limits,
    text: PositionedText,
    bytes: Meter,
    fonts: HashMap<FontKey, usize>,
    /// The number of string operands shown so far.
    show: usize,
    /// The marked-content path, outermost first.
    path: Arc<[Arc<MarkedContent>]>,
    /// The open marked-content spans.
    spans: Vec<usize>,
    /// The page, form or appearance whose `StructParents` marked content
    /// refers to.
    containers: Vec<ObjectId>,
    /// Open object occurrences and the glyph each starts at.
    objects: Vec<(ObjectId, bool, usize)>,
}

impl TextDevice {
    /// An empty device under `limits`.
    pub fn new(limits: Limits) -> Self {
        Self {
            limits,
            text: PositionedText::default(),
            bytes: Meter::new(limits.document_bytes, "text capture byte budget exceeded"),
            fonts: HashMap::new(),
            show: 0,
            path: Arc::from([]),
            spans: Vec::new(),
            containers: Vec::new(),
            objects: Vec::new(),
        }
    }

    /// The collected text.
    ///
    /// # Errors
    ///
    /// Returns `Syntax` for unbalanced marked content or object occurrences.
    pub fn finish(self) -> Result<PositionedText> {
        if !self.spans.is_empty() || !self.objects.is_empty() {
            return Err(Error::syntax("unbalanced marked content"));
        }
        Ok(self.text)
    }

    fn items(&self, count: usize, message: &'static str) -> Result<()> {
        if count >= self.limits.text_items {
            return Err(Error::limit(message));
        }
        Ok(())
    }

    fn glyph(&mut self, glyph: &PositionedGlyph, key: FontKey) -> Result<()> {
        self.items(self.text.glyphs.len(), "positioned glyph count exceeded")?;
        self.bytes.charge(
            size_of::<PositionedGlyph>() * 2 + glyph.unicode.as_deref().map_or(0, str::len),
        )?;
        let font = *self
            .fonts
            .get(&key)
            .ok_or(Error::syntax("missing extraction font"))?;
        let mut glyph = glyph.clone();
        glyph.font = font;
        glyph.marked_content = Arc::clone(&self.path);
        glyph.show = self.show;
        self.text.glyphs.push(glyph);
        Ok(())
    }

    fn begin_marked(
        &mut self,
        tag: &pagura_core::Name,
        text: Option<&crate::device::MarkedText>,
        hidden: bool,
    ) -> Result<()> {
        self.items(self.text.marked.len(), "marked content count exceeded")?;
        let text = text.cloned().unwrap_or_default();
        self.bytes.charge(
            256 + tag.len()
                + text.actual_text.as_ref().map_or(0, String::len)
                + text.alt.as_ref().map_or(0, String::len)
                + self.path.len() * 16,
        )?;
        let id = self.text.marked.len();
        let content = Arc::new(MarkedContent {
            id,
            tag: tag.clone(),
            container: self.containers.last().copied(),
            mcid: text.mcid,
            actual_text: text.actual_text,
            alt: text.alt,
        });
        let start = self.text.glyphs.len();
        self.text.marked.push(MarkedSpan {
            content: Arc::clone(&content),
            ancestors: Arc::clone(&self.path),
            glyphs: start..start,
            hidden,
        });
        let mut path = self.path.to_vec();
        path.push(content);
        self.path = path.into();
        self.spans.push(id);
        Ok(())
    }

    fn end_marked(&mut self) -> Result<()> {
        let id = self
            .spans
            .pop()
            .ok_or(Error::syntax("unbalanced extraction marked content"))?;
        let end = self.text.glyphs.len();
        let span = self
            .text
            .marked
            .get_mut(id)
            .ok_or(Error::syntax("unbalanced extraction marked content"))?;
        span.glyphs.end = end;
        self.path = Arc::clone(&span.ancestors);
        Ok(())
    }
}

impl Device for TextDevice {
    fn needs(&self) -> Needs {
        Needs::TEXT
    }

    fn begin_page(&mut self, page: &PageInfo) -> Result<()> {
        self.containers.push(page.object);
        Ok(())
    }

    fn select_font(&mut self, font: &FontInfo) -> Result<()> {
        if self.fonts.contains_key(&font.key) {
            return Ok(());
        }
        self.bytes
            .charge(256 + font.base_name.as_ref().map_or(0, pagura_core::Name::len))?;
        self.fonts.insert(font.key, self.text.fonts.len());
        self.text.fonts.push(TextFont {
            object: font.object,
            kind: font.kind,
            base_name: font.base_name.clone(),
        });
        Ok(())
    }

    fn glyphs(&mut self, run: &GlyphRun<'_>) -> Result<()> {
        self.show = self
            .show
            .checked_add(1)
            .ok_or(Error::limit("text string count exceeded"))?;
        for glyph in run.glyphs {
            if let Some(text) = &glyph.text {
                self.glyph(text, run.font.key)?;
            }
        }
        Ok(())
    }

    fn marked(&mut self, event: &Marked<'_>) -> Result<()> {
        match *event {
            Marked::Begin {
                tag, hidden, text, ..
            } => self.begin_marked(tag, text, hidden),
            Marked::End => self.end_marked(),
            Marked::ObjectBegin { object, hidden } => {
                self.objects.push((object, hidden, self.text.glyphs.len()));
                self.containers.push(object);
                Ok(())
            }
            Marked::ObjectEnd => {
                let (object, hidden, start) = self
                    .objects
                    .pop()
                    .ok_or(Error::syntax("unbalanced object occurrence"))?;
                self.containers.pop();
                self.items(self.text.objects.len(), "text object count exceeded")?;
                self.bytes.charge(size_of::<TextObject>())?;
                self.text.objects.push(TextObject {
                    marked_content: Arc::clone(&self.path),
                    object,
                    glyphs: start..self.text.glyphs.len(),
                    hidden,
                });
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests;
