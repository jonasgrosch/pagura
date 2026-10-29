//! Text objects, text state and positioning (9.3–9.4), and Type 3 glyph
//! procedures (9.6.4).

mod extract;
mod show;
mod type3;

use super::path::add_clip;
use super::{Machine, Scope, matrix};
use crate::device::Device;
use crate::model::{GroupAttributes, GroupCause};
use crate::repairs;
use pagura_content::{Op, TextItem};
use pagura_core::{Dictionary, Error, Result, Value};
use pagura_raster::{FillRule, PathBuilder, Transform};
use std::sync::Arc;

pub(crate) use show::Strings;
pub(crate) use type3::Procedure;

/// A text object (9.4.1): the text and line matrices and the glyph clip it
/// accumulates. It is not part of the saved graphics state.
#[derive(Debug, Clone)]
pub(crate) struct TextObject {
    matrix: Transform,
    line: Transform,
    clip: PathBuilder,
    clipping: bool,
    /// Whether the object's marks form a knockout group (`TK`, 9.3.8).
    knockout: bool,
    saved_depth: usize,
}

impl<D: Device + ?Sized> Machine<'_, '_, D> {
    /// The text operators.
    pub(super) fn text_operator(
        &mut self,
        op: Op<'_>,
        resources: &Dictionary,
        scope: &mut Scope,
    ) -> Result<()> {
        let text = &mut scope.state.text;
        match op {
            Op::BeginText => self.begin_text(scope)?,
            Op::EndText => self.end_text(scope)?,
            Op::CharSpacing(value) => text.character = value,
            Op::WordSpacing(value) => text.word = value,
            Op::HorizontalScaling(value) => text.horizontal = value / 100.0,
            Op::Leading(value) => text.leading = value,
            Op::Rise(value) => text.rise = value,
            Op::Font { name, size } => {
                let value =
                    super::form::resource(self.document(), resources, b"Font", name.as_bytes())?;
                self.select_font(value, size, scope)?;
            }
            Op::RenderMode(mode) => {
                let mode = u8::try_from(mode)
                    .ok()
                    .filter(|&m| m < 8)
                    .ok_or(Error::syntax("text render mode outside 0..7"))?;
                self.check_text_mode(mode, scope)?;
                if let Some(object) = &mut scope.text {
                    object.clipping |= mode >= 4;
                }
                scope.state.text.mode = mode;
            }
            Op::TextMatrix(m) => {
                let m = matrix(m);
                let object = text_object(scope)?;
                object.matrix = m;
                object.line = m;
            }
            Op::MoveText([x, y]) => text_line(scope, x, y)?,
            Op::MoveTextLeading([x, y]) => {
                text.leading = -y;
                text_line(scope, x, y)?;
            }
            Op::NextLine => {
                let leading = text.leading;
                text_line(scope, 0.0, -leading)?;
            }
            Op::ShowText(bytes) => self.show(&bytes, resources, scope)?,
            Op::ShowTextAdjusted(items) => {
                text_object(scope)?;
                let mut font = None;
                for item in items {
                    match item {
                        TextItem::Adjust(amount) => text_adjust(scope, amount)?,
                        TextItem::Show(bytes) => {
                            self.show_with(&mut font, bytes, resources, scope)?;
                        }
                    }
                }
            }
            Op::NextLineShowText(bytes) => {
                let leading = text.leading;
                text_line(scope, 0.0, -leading)?;
                self.show(&bytes, resources, scope)?;
            }
            Op::NextLineShowTextSpaced {
                word,
                character,
                text: bytes,
            } => {
                text.word = word;
                text.character = character;
                let leading = text.leading;
                text_line(scope, 0.0, -leading)?;
                self.show(&bytes, resources, scope)?;
            }
            Op::GlyphWidth(_) | Op::GlyphWidthBounds(_) => self.glyph_metrics(&op, scope)?,
            _ => return Err(Error::syntax("invalid text operator")),
        }
        Ok(())
    }

    /// `Q`, which may not cross the start of a text object.
    pub(super) fn restore(&mut self, scope: &mut Scope) -> Result<()> {
        if scope
            .text
            .as_ref()
            .is_some_and(|o| o.saved_depth == scope.saved.len())
        {
            return Err(Error::syntax("Q crosses text object boundary"));
        }
        if let Some(saved) = scope.saved.last() {
            let mode = saved.text.mode;
            self.check_text_mode(mode, scope)?;
        }
        scope.state = scope
            .saved
            .pop()
            .ok_or(Error::syntax("Q without matching q"))?;
        Ok(())
    }

    /// Selects a font, by `Tf` or a graphics state's `Font` entry.
    pub(crate) fn select_font(
        &mut self,
        value: &Value,
        size: f64,
        scope: &mut Scope,
    ) -> Result<()> {
        let font = self.session.fonts.select(value, self.position)?;
        if self.needs().text && scope.role == super::Text::Page {
            self.device.select_font(&font.info)?;
        }
        scope.state.text.font = Some(font);
        scope.state.text.size = size;
        Ok(())
    }

    /// 9.3.6: leaving a clipping mode inside a text object keeps the glyph
    /// clip accumulated so far, a reported repair.
    fn check_text_mode(&self, mode: u8, scope: &Scope) -> Result<()> {
        if mode < 4 && scope.text.as_ref().is_some_and(|o| o.clipping) {
            self.repair(
                repairs::TEXT_CLIP_RESET,
                "text clipping mode reset before ET; accumulated clip retained",
            )?;
        }
        Ok(())
    }

    fn begin_text(&mut self, scope: &mut Scope) -> Result<()> {
        if scope.text.is_some() {
            return Err(Error::syntax("nested BT"));
        }
        let knockout = scope.state.text.knockout && self.needs().paint;
        if knockout {
            self.push_lazy(
                GroupCause::TextKnockout,
                GroupAttributes {
                    knockout: true,
                    ..GroupAttributes::default()
                },
                scope,
            );
        }
        scope.text = Some(TextObject {
            matrix: Transform::IDENTITY,
            line: Transform::IDENTITY,
            clip: PathBuilder::new(),
            clipping: scope.state.text.mode >= 4,
            knockout,
            saved_depth: scope.saved.len(),
        });
        Ok(())
    }

    fn end_text(&mut self, scope: &mut Scope) -> Result<()> {
        let mut object = scope.text.take().ok_or(Error::syntax("ET without BT"))?;
        if object.saved_depth != scope.saved.len() {
            return Err(Error::syntax("ET crosses graphics state boundary"));
        }
        if object.knockout {
            self.pop_lazy()?;
        }
        if !object.clip.is_empty() {
            add_clip(
                scope,
                Arc::new(object.clip.finish()),
                FillRule::NonZero,
                self.session.limits.depth,
            )?;
        }
        Ok(())
    }
}

fn text_object(scope: &mut Scope) -> Result<&mut TextObject> {
    scope
        .text
        .as_mut()
        .ok_or(Error::syntax("text positioning or showing outside BT"))
}

/// Moves to the start of the next line, offset by `(x, y)` (9.4.2).
fn text_line(scope: &mut Scope, x: f64, y: f64) -> Result<()> {
    let object = text_object(scope)?;
    object.line = checked(Transform::translate(x, y).then(object.line))?;
    object.matrix = object.line;
    Ok(())
}

/// Moves the text position by `(x, y)` in text space.
fn text_translate(scope: &mut Scope, x: f64, y: f64) -> Result<()> {
    let object = text_object(scope)?;
    object.matrix = checked(Transform::translate(x, y).then(object.matrix))?;
    Ok(())
}

/// A `TJ` adjustment in thousandths of text space units (9.4.3).
fn text_adjust(scope: &mut Scope, amount: f64) -> Result<()> {
    let text = &scope.state.text;
    let font = text
        .font
        .as_ref()
        .ok_or(Error::syntax("text showing requires a font"))?;
    let amount = -amount / 1000.0 * text.size;
    if font.vertical() {
        text_translate(scope, 0.0, amount)
    } else {
        let horizontal = text.horizontal;
        text_translate(scope, amount * horizontal, 0.0)
    }
}

/// A text transform that must stay finite.
pub(crate) fn checked(matrix: Transform) -> Result<Transform> {
    if matrix.is_finite() {
        Ok(matrix)
    } else {
        Err(Error::syntax("text matrix arithmetic overflow"))
    }
}
