//! Text state, positioning and outline painting (ISO 32000-2, 9.3–9.4).

mod bounds;
pub(crate) mod cache;
mod type3;

use super::{
    Arc, Compiler, Dictionary, Error, FillRule, PathBuilder, Result, Transform, Value, matrix,
    number, resource,
};
use pagura_core::Position;
use pagura_raster::{Path, Segment};
use pagura_text::{Glyph, GlyphShape, PdfFont, WritingMode};
use std::{cell::RefCell, rc::Rc};

#[derive(Debug)]
pub(super) struct TextState {
    font: Option<Arc<PdfFont>>,
    size: f64,
    character: f64,
    word: f64,
    horizontal: f64,
    leading: f64,
    rise: f64,
    mode: u8,
    knockout: bool,
    pub(super) object: Option<TextObject>,
    pub(super) procedure: Option<type3::Procedure>,
    pub(super) cache: Rc<RefCell<cache::Cache>>,
}
// Text matrices and accumulated clipping belong to the content context,
// rather than the saved graphics state (9.4.1). Detached forms and masks
// inherit text parameters, never an enclosing text object's runtime.
impl Clone for TextState {
    fn clone(&self) -> Self {
        Self {
            font: self.font.clone(),
            size: self.size,
            character: self.character,
            word: self.word,
            horizontal: self.horizontal,
            leading: self.leading,
            rise: self.rise,
            mode: self.mode,
            knockout: self.knockout,
            object: None,
            procedure: self.procedure.clone(),
            cache: Rc::clone(&self.cache),
        }
    }
}
impl Default for TextState {
    fn default() -> Self {
        Self {
            font: None,
            size: 0.0,
            character: 0.0,
            word: 0.0,
            horizontal: 1.0,
            leading: 0.0,
            rise: 0.0,
            mode: 0,
            knockout: true,
            object: None,
            procedure: None,
            cache: Rc::default(),
        }
    }
}
#[derive(Debug, Clone)]
pub(super) struct TextObject {
    matrix: Transform,
    line: Transform,
    clip: PathBuilder,
    clipping: bool,
    start: usize,
    knockout: bool,
    position: Position,
    pub(super) saved_depth: usize,
}

impl Compiler<'_> {
    pub(super) fn save_graphics(&mut self) -> Result<()> {
        if self.saved.len() >= self.document.limits().depth {
            return Err(Error::limit("graphics state nesting exceeded"));
        }
        let object = self.state.text.object.take();
        self.saved.push(self.state.clone());
        self.state.text.object = object;
        Ok(())
    }
    pub(super) fn restore_graphics(&mut self) -> Result<()> {
        if self
            .state
            .text
            .object
            .as_ref()
            .is_some_and(|o| o.saved_depth == self.saved.len())
        {
            return Err(Error::syntax("Q crosses text object boundary"));
        }
        let object = self.state.text.object.take();
        let procedure = self.state.text.procedure.clone();
        self.state = self
            .saved
            .pop()
            .ok_or(Error::syntax("Q without matching q"))?;
        self.state.text.object = object;
        self.state.text.procedure = procedure;
        Ok(())
    }
    pub(super) fn text_operator(
        &mut self,
        word: &[u8],
        v: &[Value],
        resources: &Dictionary,
    ) -> Result<()> {
        match word {
            b"BT" => self.begin_text()?,
            b"ET" => self.end_text()?,
            b"Tc" => self.state.text.character = number(v, 0)?,
            b"Tw" => self.state.text.word = number(v, 0)?,
            b"Tz" => self.state.text.horizontal = number(v, 0)? / 100.0,
            b"TL" => self.state.text.leading = number(v, 0)?,
            b"Ts" => self.state.text.rise = number(v, 0)?,
            b"Tf" => {
                let name = v
                    .first()
                    .and_then(Value::as_name)
                    .ok_or(Error::syntax("font name expected"))?;
                let value = resource(self.document, resources, b"Font", name.as_bytes())?;
                self.select_font(value, number(v, 1)?)?;
            }
            b"Tr" => {
                let mode = v
                    .first()
                    .and_then(Value::as_integer)
                    .and_then(|v| u8::try_from(v).ok())
                    .filter(|&v| v < 8)
                    .ok_or(Error::syntax("text render mode outside 0..7"))?;
                if let Some(object) = &mut self.state.text.object {
                    if object.clipping && mode < 4 {
                        return Err(Error::syntax("text clipping mode reset before ET"));
                    }
                    object.clipping |= mode >= 4;
                }
                self.state.text.mode = mode;
            }
            b"Tm" => {
                let m = matrix(v)?;
                let object = self.text_object()?;
                object.matrix = m;
                object.line = m;
            }
            b"Td" | b"TD" => {
                let (x, y) = (number(v, 0)?, number(v, 1)?);
                if word == b"TD" {
                    self.state.text.leading = -y;
                }
                self.text_line(x, y)?;
            }
            b"T*" => self.text_line(0.0, -self.state.text.leading)?,
            b"Tj" => self.show_value(&v[0], resources)?,
            b"TJ" => {
                self.text_object()?;
                let array = v[0].as_array().ok_or(Error::syntax("TJ array expected"))?;
                for value in array {
                    if let Some(adjustment) = value.as_number() {
                        self.text_adjust(adjustment)?;
                    } else {
                        self.show_value(value, resources)?;
                    }
                }
            }
            b"'" | b"\"" => {
                let string = if word == b"\"" {
                    self.state.text.word = number(v, 0)?;
                    self.state.text.character = number(v, 1)?;
                    &v[2]
                } else {
                    &v[0]
                };
                self.text_line(0.0, -self.state.text.leading)?;
                self.show_value(string, resources)?;
            }
            b"d0" | b"d1" => self.glyph_metrics(word, v)?,
            _ => return Err(Error::syntax("invalid text operator")),
        }
        Ok(())
    }

    fn begin_text(&mut self) -> Result<()> {
        if self.state.text.object.is_some() {
            return Err(Error::syntax("nested BT"));
        }
        self.state.text.object = Some(TextObject {
            matrix: Transform::IDENTITY,
            line: Transform::IDENTITY,
            clip: PathBuilder::new(),
            clipping: self.state.text.mode >= 4,
            start: self.list.commands.len(),
            knockout: self.state.text.knockout,
            position: self.position,
            saved_depth: self.saved.len(),
        });
        Ok(())
    }
    fn end_text(&mut self) -> Result<()> {
        let mut object = self
            .state
            .text
            .object
            .take()
            .ok_or(Error::syntax("ET without BT"))?;
        if object.saved_depth != self.saved.len() {
            return Err(Error::syntax("ET crosses graphics state boundary"));
        }
        if object.knockout && self.list.commands.len() > object.start {
            self.paint_charge(0, 1)?;
            let commands = self.list.commands.split_off(object.start);
            self.list
                .commands
                .push(super::Command::Group(crate::transparency::Group {
                    position: object.position,
                    bounds: bounds::bounds(&commands),
                    commands,
                    attributes: crate::transparency::Attributes {
                        knockout: true,
                        ..Default::default()
                    },
                    composite: crate::transparency::Composite::opaque(),
                }));
        }
        if !object.clip.is_empty() {
            self.add_clip(Arc::new(object.clip.finish()), FillRule::NonZero)?;
        }
        Ok(())
    }
    fn text_object(&mut self) -> Result<&mut TextObject> {
        self.state
            .text
            .object
            .as_mut()
            .ok_or(Error::syntax("text positioning or showing outside BT"))
    }
    fn text_line(&mut self, x: f64, y: f64) -> Result<()> {
        let object = self.text_object()?;
        object.line = checked(Transform::translate(x, y).then(object.line))?;
        object.matrix = object.line;
        Ok(())
    }
    fn text_translate(&mut self, x: f64, y: f64) -> Result<()> {
        let object = self.text_object()?;
        object.matrix = checked(Transform::translate(x, y).then(object.matrix))?;
        Ok(())
    }
    fn text_adjust(&mut self, amount: f64) -> Result<()> {
        let text = &self.state.text;
        let font = text
            .font
            .as_ref()
            .ok_or(Error::syntax("text showing requires a font"))?;
        let amount = -amount / 1000.0 * text.size;
        if font.writing_mode() == WritingMode::Vertical {
            self.text_translate(0.0, amount)
        } else {
            self.text_translate(amount * text.horizontal, 0.0)
        }
    }
    fn show_value(&mut self, value: &Value, resources: &Dictionary) -> Result<()> {
        self.text_object()?;
        let bytes = value
            .as_string()
            .ok_or(Error::syntax("text string expected"))?;
        let font = self
            .state
            .text
            .font
            .clone()
            .ok_or(Error::syntax("text showing requires a font"))?;
        let glyphs = font.decode(bytes).map_err(|mut e| {
            e.position = self.position;
            e
        })?;
        self.operations = self
            .operations
            .checked_add(glyphs.len())
            .filter(|&n| n <= self.document.limits().operations)
            .ok_or(Error::limit("text glyph work exceeded"))?;
        for glyph in glyphs {
            self.show_glyph(&font, glyph, resources)?;
        }
        Ok(())
    }
    fn show_glyph(&mut self, font: &PdfFont, glyph: Glyph, resources: &Dictionary) -> Result<()> {
        let text = &self.state.text;
        let matrix = text
            .object
            .as_ref()
            .ok_or(Error::syntax("glyph outside text object"))?
            .matrix;
        let origin = glyph
            .vertical
            .map_or((0.0, 0.0), |v| (-v.origin.0, -v.origin.1));
        let transform = checked(
            Transform::translate(origin.0, origin.1)
                .then(Transform::new(
                    text.size * text.horizontal,
                    0.0,
                    0.0,
                    text.size,
                    0.0,
                    text.rise,
                ))
                .then(matrix)
                .then(self.state.ctm),
        )?;
        let size = text.size;
        let mode = text.mode;
        let spacing = text.character + if glyph.word_space { text.word } else { 0.0 };
        let (dx, dy) = glyph.vertical.map_or(
            ((glyph.advance * size + spacing) * text.horizontal, 0.0),
            |v| (0.0, v.advance * size + spacing),
        );
        // Zero-sized text neither paints nor clips (Table 103), but still spaces.
        if size != 0.0 {
            match glyph.shape {
                GlyphShape::Outline {
                    program,
                    glyph,
                    matrix,
                } if mode != 3 => {
                    let key = (Arc::as_ptr(&program) as usize, glyph.0);
                    let shared = self.profiles.fonts.and_then(|fonts| {
                        fonts
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .outlines
                            .get(&key)
                            .cloned()
                    });
                    let cached = shared
                        .or_else(|| self.state.text.cache.borrow().outlines.get(&key).cloned());
                    let path = if let Some(path) = cached {
                        path
                    } else {
                        let path = program.outline(glyph).map_err(|mut error| {
                            // A font-buffer offset is not an offset in the
                            // calling content sequence. Retain its call site.
                            error.position = self.position;
                            error
                        })?;
                        self.paint_charge(path_bytes(&path), 1)?;
                        let path = Arc::new(path);
                        if let Some(fonts) = self.profiles.fonts {
                            fonts
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .outline(font, key, Arc::clone(&path));
                        }
                        self.state
                            .text
                            .cache
                            .borrow_mut()
                            .outlines
                            .insert(key, Arc::clone(&path));
                        path
                    };
                    let transform = checked(matrix.then(transform))?;
                    self.outline_text(&path, transform, mode)?;
                }
                GlyphShape::Type3 { procedure, matrix } if mode != 3 && mode != 7 => {
                    self.type3_glyph(
                        font,
                        procedure,
                        checked(matrix.then(transform))?,
                        resources,
                        (glyph.advance, matrix.a),
                    )?;
                }
                _ => {}
            }
        }
        self.text_translate(dx, dy)
    }
    fn outline_text(&mut self, outline: &Path, transform: Transform, mode: u8) -> Result<()> {
        if outline.is_empty() {
            return Ok(());
        }
        // Painting plus clipping retains two separate paths; clip-only and
        // paint-only modes retain one (9.3.6).
        let copies = if (4..=6).contains(&mode) { 2 } else { 1 };
        self.paint_charge(path_bytes(outline).saturating_mul(copies), 0)?;
        let path = outline.transformed(transform);
        validate_path(&path)?;
        if mode >= 4 {
            let limit = self.document.limits().path_segments;
            let clip = &mut self.text_object()?.clip;
            append(clip, &path, limit)?;
        }
        let paint = match mode % 4 {
            0 => Some(b"f".as_slice()),
            1 => Some(b"S".as_slice()),
            2 => Some(b"B".as_slice()),
            _ => None,
        };
        if let Some(operator) = paint {
            let old = std::mem::take(&mut self.path);
            append(&mut self.path, &path, self.document.limits().path_segments)?;
            self.paint(operator)?;
            self.path = old;
        }
        Ok(())
    }
}

fn checked(matrix: Transform) -> Result<Transform> {
    if matrix.is_finite() {
        Ok(matrix)
    } else {
        Err(Error::syntax("text matrix arithmetic overflow"))
    }
}
fn path_bytes(path: &Path) -> usize {
    path.segments().len().saturating_mul(size_of::<Segment>())
}
fn validate_path(path: &Path) -> Result<()> {
    for segment in path.segments() {
        match *segment {
            Segment::MoveTo(p) | Segment::LineTo(p) => super::finite(p)?,
            Segment::CubicTo(a, b, c) => {
                super::finite(a)?;
                super::finite(b)?;
                super::finite(c)?;
            }
            Segment::Close => {}
        }
    }
    Ok(())
}
fn append(builder: &mut PathBuilder, path: &Path, limit: usize) -> Result<()> {
    if builder
        .len()
        .checked_add(path.segments().len())
        .is_none_or(|n| n > limit)
    {
        return Err(Error::limit("text path segment budget exceeded"));
    }
    for segment in path.segments() {
        match *segment {
            Segment::MoveTo(p) => {
                builder.move_to(p);
            }
            Segment::LineTo(p) => {
                builder.line_to(p);
            }
            Segment::CubicTo(a, b, c) => {
                builder.cubic_to(a, b, c);
            }
            Segment::Close => {
                builder.close();
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
