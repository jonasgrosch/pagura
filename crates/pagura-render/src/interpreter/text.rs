//! Text state, positioning and outline painting (ISO 32000-2, 9.3–9.4).

mod bounds;
pub(crate) mod cache;
pub(super) mod extract;
mod type3;

use super::{
    Arc, Compiler, Dictionary, Error, FillRule, PathBuilder, Result, Transform, Value, matrix,
    number, resource,
};
use pagura_core::Position;
use pagura_raster::{Path, Segment};
use pagura_text::{Glyph, GlyphShape, PdfFont, WritingMode};
use std::{cell::RefCell, rc::Rc};

// Bound the placement and source tables retained by a single run.
const MAX_GLYPHS_PER_RUN: usize = 65_535;

#[derive(Debug)]
pub(super) struct TextState {
    font: Option<Arc<cache::CachedFont>>,
    pub(super) run: Option<usize>,
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
            run: None,
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
            run: None,
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
        if let Some(saved) = self.saved.last() {
            self.check_text_mode(saved.text.mode)?;
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

    pub(super) fn text_operator<const EXTRACT: bool>(
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
                self.select_font_mode::<EXTRACT>(value, number(v, 1)?)?;
            }
            b"Tr" => {
                let mode = v
                    .first()
                    .and_then(Value::as_integer)
                    .and_then(|v| u8::try_from(v).ok())
                    .filter(|&v| v < 8)
                    .ok_or(Error::syntax("text render mode outside 0..7"))?;
                self.check_text_mode(mode)?;
                if let Some(object) = &mut self.state.text.object {
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
            b"Tj" => self.show_value::<EXTRACT>(&v[0], resources)?,
            b"TJ" => {
                self.text_object()?;
                let array = v[0].as_array().ok_or(Error::syntax("TJ array expected"))?;
                for value in array {
                    if let Some(adjustment) = value.as_number() {
                        self.text_adjust(adjustment)?;
                    } else {
                        self.show_value::<EXTRACT>(value, resources)?;
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
                self.show_value::<EXTRACT>(string, resources)?;
            }
            b"d0" | b"d1" => self.glyph_metrics(word, v)?,
            _ => return Err(Error::syntax("invalid text operator")),
        }
        Ok(())
    }

    fn check_text_mode(&self, mode: u8) -> Result<()> {
        if mode < 4 && self.state.text.object.as_ref().is_some_and(|o| o.clipping) {
            self.repair(
                super::tags::TEXT_CLIP_RESET,
                "text clipping mode reset before ET; accumulated clip retained",
            )?;
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
        if font.font.writing_mode() == WritingMode::Vertical {
            self.text_translate(0.0, amount)
        } else {
            self.text_translate(amount * text.horizontal, 0.0)
        }
    }

    fn show_value<const EXTRACT: bool>(
        &mut self,
        value: &Value,
        resources: &Dictionary,
    ) -> Result<()> {
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
        let glyphs = font.font.decode(bytes).map_err(|mut e| {
            e.position = self.position;
            e
        })?;
        self.operations = self
            .operations
            .checked_add(glyphs.len())
            .filter(|&n| n <= self.document.limits().operations)
            .ok_or(Error::limit("text glyph work exceeded"))?;
        if EXTRACT {
            self.extract_show()?;
        }
        for glyph in glyphs {
            self.show_glyph::<EXTRACT>(&font, glyph, resources)?;
        }
        Ok(())
    }

    fn show_glyph<const EXTRACT: bool>(
        &mut self,
        font: &cache::CachedFont,
        glyph: Glyph,
        resources: &Dictionary,
    ) -> Result<()> {
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
        if EXTRACT {
            self.extract_glyph(font, &glyph, matrix, transform)?;
        }
        // Zero-sized text neither paints nor clips (Table 103), but still spaces.
        if size != 0.0 {
            match glyph.shape {
                GlyphShape::Outline {
                    program,
                    glyph,
                    matrix,
                } if mode != 3 => {
                    let path = self.glyph_outline(font, glyph, &program)?;
                    let transform = checked(matrix.then(transform))?;
                    self.outline_text(path, transform, mode)?;
                }
                GlyphShape::Type3 { procedure, matrix } if mode != 3 && mode != 7 => {
                    self.type3_glyph(
                        &font.font,
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

    fn outline_text(&mut self, outline: Arc<Path>, transform: Transform, mode: u8) -> Result<()> {
        if outline.is_empty() {
            return Ok(());
        }
        if matches!(mode, 0 | 4) {
            if mode == 4 {
                self.paint_charge(path_bytes(&outline), 0)?;
                let path = outline.transformed(transform);
                validate_path(&path)?;
                let limit = self.document.limits().path_segments;
                append(&mut self.text_object()?.clip, &path, limit)?;
            }
            if self.visible {
                self.append_glyph(outline, transform)?;
            }
            return Ok(());
        }
        self.state.text.run = None;
        // Painting plus clipping retains two separate paths; clip-only and
        // paint-only modes retain one (9.3.6).
        let copies = if (4..=6).contains(&mode) { 2 } else { 1 };
        self.paint_charge(path_bytes(&outline).saturating_mul(copies), 0)?;
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
    fn append_glyph(&mut self, outline: Arc<Path>, transform: Transform) -> Result<()> {
        self.marks = self
            .marks
            .checked_add(1)
            .filter(|&n| n <= self.document.limits().display_commands)
            .ok_or(Error::limit("display list budget exceeded"))?;
        // Bound placement/source/index capacities before growing the run.
        self.paint_charge(crate::display::GlyphRun::placement_bytes(), 0)?;
        let run = self
            .state
            .text
            .run
            .filter(|&i| i + 1 == self.list.commands.len());
        let run = run.filter(|&i| match &self.list.commands[i] {
            super::Command::Glyphs(run) => run.glyphs.len() < MAX_GLYPHS_PER_RUN,
            _ => false,
        });
        let index = if let Some(i) = run {
            i
        } else {
            self.paint_charge(size_of::<crate::display::GlyphRun>(), 0)?;
            let index = self.list.commands.len();
            self.list.commands.push(super::Command::Glyphs(Box::new(
                crate::display::GlyphRun::new(super::PathCommand {
                    path: Arc::default(),
                    mark: super::Mark::Fill(FillRule::NonZero),
                    clip: self.state.clip.clone(),
                    color: self.state.fill.clone(),
                    position: self.position,
                    transfer: self.state.transfer.clone(),
                    composite: self.state.composite.clone(),
                }),
            )));
            self.state.text.run = Some(index);
            index
        };
        let super::Command::Glyphs(run) = &mut self.list.commands[index] else {
            return Err(Error::invalid_argument("invalid text run"));
        };
        run.push(outline, transform, self.position)
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
            Segment::CubicTo {
                c1: a,
                c2: b,
                end: c,
            } => {
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
    builder.append(path, Transform::IDENTITY);
    Ok(())
}

#[cfg(test)]
mod tests;
