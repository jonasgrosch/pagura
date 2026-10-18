//! Text state, positioning and outline painting (ISO 32000-2, 9.3–9.4).

mod bounds;
pub(crate) mod cache;
pub(super) mod extract;
pub(super) mod type3;

use super::{
    Arc, Compiler, Dictionary, Error, FillRule, PathBuilder, Result, Transform, Value, resource,
};
use crate::operators::{Painting, Target, TextOperation};
use pagura_core::Position;
use pagura_raster::{Path, Segment};
use pagura_text::{Glyph, GlyphShape, PdfFont, WritingMode};

// Bound the placement and source tables retained by a single run.
const MAX_GLYPHS_PER_RUN: usize = 65_535;

/// Text state parameters (9.3, Table 102), part of the graphics state.
#[derive(Debug, Clone)]
pub(super) struct TextState {
    font: Option<Arc<cache::CachedFont>>,
    size: f64,
    character: f64,
    word: f64,
    horizontal: f64,
    leading: f64,
    rise: f64,
    mode: RenderMode,
    pub(super) knockout: bool,
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
            mode: RenderMode::Fill,
            knockout: true,
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
        self.saved.push(self.state.clone());
        Ok(())
    }

    pub(super) fn restore_graphics(&mut self) -> Result<()> {
        if self
            .text_object
            .as_ref()
            .is_some_and(|o| o.saved_depth == self.saved.len())
        {
            return Err(Error::syntax("Q crosses text object boundary"));
        }
        if let Some(saved) = self.saved.last() {
            self.check_text_mode(saved.text.mode)?;
        }
        self.state = self
            .saved
            .pop()
            .ok_or(Error::syntax("Q without matching q"))?;
        Ok(())
    }

    pub(super) fn text_operator(
        &mut self,
        operation: TextOperation<'_>,
        resources: &Dictionary,
    ) -> Result<()> {
        match operation {
            TextOperation::Begin => self.begin_text()?,
            TextOperation::End => self.end_text()?,
            TextOperation::CharacterSpacing(spacing) => self.state.text.character = spacing,
            TextOperation::WordSpacing(spacing) => self.state.text.word = spacing,
            TextOperation::HorizontalScaling(scale) => self.state.text.horizontal = scale / 100.0,
            TextOperation::Leading(leading) => self.state.text.leading = leading,
            TextOperation::Rise(rise) => self.state.text.rise = rise,
            TextOperation::Font { name, size } => {
                let value = resource(self.document, resources, b"Font", name.as_bytes())?;
                self.select_font(value, size)?;
            }
            TextOperation::RenderMode(mode) => {
                let mode = RenderMode::try_from(mode)?;
                self.check_text_mode(mode)?;
                if let Some(object) = &mut self.text_object {
                    object.clipping |= mode.clips();
                }
                self.state.text.mode = mode;
            }
            TextOperation::Matrix(m) => {
                let object = self.open_text()?;
                object.matrix = m;
                object.line = m;
            }
            TextOperation::Move { x, y, set_leading } => {
                if set_leading {
                    self.state.text.leading = -y;
                }
                self.text_line(x, y)?;
            }
            TextOperation::NextLine => self.text_line(0.0, -self.state.text.leading)?,
            TextOperation::Show(text) => self.show_text(text)?,
            TextOperation::ShowArray(array) => {
                self.open_text()?;
                for value in array {
                    match value {
                        Value::String(text) => self.show_text(text)?,
                        adjustment => {
                            let adjustment = adjustment
                                .as_number()
                                .ok_or(Error::syntax("TJ array requires numbers or strings"))?;
                            self.text_adjust(adjustment)?;
                        }
                    }
                }
            }
            TextOperation::NextLineShow(text) => {
                self.text_line(0.0, -self.state.text.leading)?;
                self.show_text(text)?;
            }
            TextOperation::NextLineShowSpaced {
                word,
                character,
                text,
            } => {
                self.state.text.word = word;
                self.state.text.character = character;
                self.text_line(0.0, -self.state.text.leading)?;
                self.show_text(text)?;
            }
        }
        Ok(())
    }

    fn check_text_mode(&self, mode: RenderMode) -> Result<()> {
        if !mode.clips() && self.text_object.as_ref().is_some_and(|o| o.clipping) {
            self.repair(
                crate::repairs::TEXT_CLIP_RESET,
                "text clipping mode reset before ET; accumulated clip retained",
            )?;
        }
        Ok(())
    }

    fn begin_text(&mut self) -> Result<()> {
        if self.text_object.is_some() {
            return Err(Error::syntax("nested BT"));
        }
        self.text_object = Some(TextObject {
            matrix: Transform::IDENTITY,
            line: Transform::IDENTITY,
            clip: PathBuilder::new(),
            clipping: self.state.text.mode.clips(),
            start: self.commands.len(),
            knockout: self.state.text.knockout,
            position: self.position,
            saved_depth: self.saved.len(),
        });
        Ok(())
    }

    fn end_text(&mut self) -> Result<()> {
        let mut object = self
            .text_object
            .take()
            .ok_or(Error::syntax("ET without BT"))?;
        if object.saved_depth != self.saved.len() {
            return Err(Error::syntax("ET crosses graphics state boundary"));
        }
        if object.knockout && self.commands.len() > object.start {
            self.paint_charge(0, 1)?;
            let commands = self.commands.split_off(object.start);
            self.commands
                .push(super::Command::Group(crate::transparency::Group {
                    position: object.position,
                    bounds: bounds::bounds(&commands),
                    commands,
                    attributes: crate::transparency::Attributes {
                        knockout: true,
                        ..Default::default()
                    },
                    composite: crate::transparency::Composite::default(),
                }));
        }
        if !object.clip.is_empty() {
            self.add_clip(Arc::new(object.clip.finish()), FillRule::NonZero)?;
        }
        Ok(())
    }

    fn open_text(&mut self) -> Result<&mut TextObject> {
        self.text_object
            .as_mut()
            .ok_or(Error::syntax("text positioning or showing outside BT"))
    }

    fn text_line(&mut self, x: f64, y: f64) -> Result<()> {
        let object = self.open_text()?;
        object.line = checked(Transform::translate(x, y).then(object.line))?;
        object.matrix = object.line;
        Ok(())
    }

    fn text_translate(&mut self, x: f64, y: f64) -> Result<()> {
        let object = self.open_text()?;
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

    fn show_text(&mut self, bytes: &[u8]) -> Result<()> {
        self.open_text()?;
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
        self.session
            .charge_operations(glyphs.len(), "text glyph work exceeded")?;
        self.extract_show()?;
        for glyph in glyphs {
            self.show_glyph(&font, glyph)?;
        }
        Ok(())
    }

    fn show_glyph(&mut self, font: &cache::CachedFont, glyph: Glyph) -> Result<()> {
        let matrix = self
            .text_object
            .as_ref()
            .ok_or(Error::syntax("glyph outside text object"))?
            .matrix;
        let text = &self.state.text;
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
        self.extract_glyph(font, &glyph, matrix, transform)?;
        // Zero-sized text neither paints nor clips (Table 103), but still spaces.
        if size != 0.0 {
            match glyph.shape {
                GlyphShape::Outline {
                    program,
                    glyph,
                    matrix,
                } if mode != RenderMode::Invisible => {
                    let path = self.glyph_outline(font, glyph, &program)?;
                    let transform = checked(matrix.then(transform))?;
                    self.outline_text(path, transform, mode)?;
                }
                // Type 3 glyphs only paint; they add nothing to the clip (9.3.6).
                GlyphShape::Type3 { procedure, matrix } if mode.painting().is_some() => {
                    self.type3_glyph(
                        &font.font,
                        procedure,
                        checked(matrix.then(transform))?,
                        type3::Advance {
                            advance: glyph.advance,
                            scale: matrix.a,
                        },
                    )?;
                }
                _ => {}
            }
        }
        self.text_translate(dx, dy)
    }

    fn outline_text(
        &mut self,
        outline: Arc<Path>,
        transform: Transform,
        mode: RenderMode,
    ) -> Result<()> {
        if outline.is_empty() {
            return Ok(());
        }
        // Fill and fill-and-clip glyphs join the context's glyph run (9.3.8).
        if matches!(mode, RenderMode::Fill | RenderMode::FillClip) {
            if mode == RenderMode::FillClip {
                self.paint_charge(path_bytes(&outline), 0)?;
                let path = outline.transformed(transform);
                validate_path(&path)?;
                let limit = self.document.limits().path_segments;
                append(&mut self.open_text()?.clip, &path, limit)?;
            }
            if self.visible {
                self.append_glyph(outline, transform)?;
            }
            return Ok(());
        }
        self.text_run = None;
        // Painting plus clipping retains two separate paths; clip-only and
        // paint-only modes retain one (9.3.6).
        let copies = if mode.clips() && mode.painting().is_some() {
            2
        } else {
            1
        };
        self.paint_charge(path_bytes(&outline).saturating_mul(copies), 0)?;
        let path = outline.transformed(transform);
        validate_path(&path)?;
        if mode.clips() {
            let limit = self.document.limits().path_segments;
            let clip = &mut self.open_text()?.clip;
            append(clip, &path, limit)?;
        }
        if let Some(painting) = mode.painting() {
            let old = std::mem::take(&mut self.path);
            append(&mut self.path, &path, self.document.limits().path_segments)?;
            self.paint(painting)?;
            self.path = old;
        }
        Ok(())
    }
    /// Appends a fill-mode glyph to the open run of this context, starting a
    /// new run after any other operation or at the run's size bound.
    fn append_glyph(&mut self, outline: Arc<Path>, transform: Transform) -> Result<()> {
        self.session.charge_marks(1)?;
        // Bound placement/source/index capacities before growing the run.
        self.paint_charge(crate::display::GlyphRun::placement_bytes(), 0)?;
        let open = self.text_run.filter(|&i| i + 1 == self.commands.len());
        if let Some(super::Command::Glyphs(run)) = open.and_then(|i| self.commands.get_mut(i))
            && run.glyphs.len() < MAX_GLYPHS_PER_RUN
        {
            return run.push(outline, transform, self.position);
        }
        self.paint_charge(size_of::<crate::display::GlyphRun>(), 0)?;
        let mut run = Box::new(crate::display::GlyphRun::new(super::PathCommand {
            path: Arc::default(),
            mark: super::Mark::Fill(FillRule::NonZero),
            clip: self.state.clip.clone(),
            color: self.state.fill.color.clone(),
            position: self.position,
            transfer: self.state.transfer.clone(),
            composite: self.state.composite(Target::Fill),
        }));
        run.push(outline, transform, self.position)?;
        self.text_run = Some(self.commands.len());
        self.commands.push(super::Command::Glyphs(run));
        Ok(())
    }
}

/// Text rendering modes (9.3.6, Table 104).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(super) enum RenderMode {
    Fill = 0,
    Stroke = 1,
    FillStroke = 2,
    Invisible = 3,
    FillClip = 4,
    StrokeClip = 5,
    FillStrokeClip = 6,
    Clip = 7,
}
impl TryFrom<i64> for RenderMode {
    type Error = Error;
    fn try_from(value: i64) -> Result<Self> {
        Ok(match value {
            0 => Self::Fill,
            1 => Self::Stroke,
            2 => Self::FillStroke,
            3 => Self::Invisible,
            4 => Self::FillClip,
            5 => Self::StrokeClip,
            6 => Self::FillStrokeClip,
            7 => Self::Clip,
            _ => return Err(Error::syntax("text render mode outside 0..7")),
        })
    }
}
impl RenderMode {
    /// How glyph outlines are painted, if at all.
    pub(super) const fn painting(self) -> Option<Painting> {
        match self {
            Self::Fill | Self::FillClip => Some(Painting::FILL),
            Self::Stroke | Self::StrokeClip => Some(Painting::STROKE),
            Self::FillStroke | Self::FillStrokeClip => Some(Painting::FILL_STROKE),
            Self::Invisible | Self::Clip => None,
        }
    }
    /// Whether glyph outlines are added to the text clip.
    pub(super) const fn clips(self) -> bool {
        matches!(
            self,
            Self::FillClip | Self::StrokeClip | Self::FillStrokeClip | Self::Clip
        )
    }
    /// The Table 104 code.
    pub(super) const fn code(self) -> u8 {
        self as u8
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
