//! Showing strings (9.4.3): glyph placement, outlines, the text clip
//! (9.3.6), and one glyph run per string operand for the device.

use super::{Machine, Scope, checked, text_object, text_translate};
use crate::device::{Device, Glyph, GlyphPaint, GlyphRun, MarkState, OutlineKey};
use crate::fonts::LoadedFont;
use crate::machine::Text;
use crate::machine::path::finite;
use pagura_core::{Dictionary, Error, Result};
use pagura_raster::{Path, PathBuilder, Segment, Transform};
use pagura_text::GlyphShape;
use std::sync::Arc;

impl<D: Device + ?Sized> Machine<'_, '_, D> {
    /// Shows one string operand.
    pub(super) fn show(
        &mut self,
        bytes: &[u8],
        resources: &Dictionary,
        scope: &mut Scope,
    ) -> Result<()> {
        let mut font = None;
        self.show_with(&mut font, bytes, resources, scope)
    }

    /// Shows one string operand in the current font, which `font` keeps
    /// for the strings of one `TJ` array once the first is shown.
    pub(super) fn show_with(
        &mut self,
        font: &mut Option<Arc<LoadedFont>>,
        bytes: &[u8],
        resources: &Dictionary,
        scope: &mut Scope,
    ) -> Result<()> {
        text_object(scope)?;
        let font = match font {
            Some(font) => font,
            None => font.insert(
                scope
                    .state
                    .text
                    .font
                    .clone()
                    .ok_or(Error::syntax("text showing requires a font"))?,
            ),
        };
        let needs = self.needs();
        let text = needs.text && scope.role == Text::Page;
        // The buffers of the previous string are reused; a string shown by
        // a Type 3 glyph inside this one takes fresh ones.
        let mut decoded = std::mem::take(&mut self.session.decoded);
        let mut glyphs = std::mem::take(&mut self.session.placed);
        let result = self.show_glyphs(
            font,
            bytes,
            text,
            (&mut decoded, &mut glyphs),
            resources,
            scope,
        );
        decoded.clear();
        glyphs.clear();
        self.session.decoded = decoded;
        self.session.placed = glyphs;
        result
    }

    fn show_glyphs(
        &mut self,
        font: &LoadedFont,
        bytes: &[u8],
        text: bool,
        (decoded, glyphs): (&mut Vec<pagura_text::Glyph>, &mut Vec<Glyph>),
        resources: &Dictionary,
        scope: &mut Scope,
    ) -> Result<()> {
        font.font
            .decode_into(bytes, text, decoded)
            .map_err(|mut e| {
                e.position = self.position;
                e
            })?;
        self.session
            .tokens
            .charge(decoded.len())
            .map_err(|_| Error::limit("text glyph work exceeded"))?;
        glyphs.reserve(decoded.len());
        let run = Run::new(scope, self.needs().outlines);
        for glyph in decoded.iter() {
            let placed = self.glyph(font, glyph, text, &run, resources, scope)?;
            glyphs.push(placed);
        }
        self.emit_run(font, glyphs, text, scope)
    }

    /// Places one glyph: its record for text, its outline or procedure for
    /// paint, its part of the text clip, and the advance (9.4.4).
    fn glyph(
        &mut self,
        font: &LoadedFont,
        glyph: &pagura_text::Glyph,
        text: bool,
        run: &Run,
        resources: &Dictionary,
        scope: &mut Scope,
    ) -> Result<Glyph> {
        let matrix = scope
            .text
            .as_ref()
            .ok_or(Error::syntax("glyph outside text object"))?
            .matrix;
        let scaled = match glyph.vertical {
            Some(v) => Transform::translate(-v.origin.0, -v.origin.1).then(run.scale),
            None => run.scaled,
        };
        let transform = checked(scaled.then(matrix).then(run.ctm))?;
        let spacing = run.character + if glyph.word_space { run.word } else { 0.0 };
        let (dx, dy) = glyph.vertical.map_or(
            ((glyph.advance * run.size + spacing) * run.horizontal, 0.0),
            |v| (0.0, v.advance * run.size + spacing),
        );
        let positioned = if text {
            Some(Box::new(
                self.positioned(font, glyph, matrix, transform, scope)?,
            ))
        } else {
            None
        };
        let mut placed = Glyph {
            code: glyph.code,
            outline_key: OutlineKey {
                font: font.info.key,
                glyph: glyph.code,
            },
            transform,
            outline: None,
            text: positioned,
            position: self.position,
        };
        let mode = run.mode;
        // Zero-sized text neither paints nor clips (Table 103), but advances.
        if run.size != 0.0 && run.outlines {
            match &glyph.shape {
                GlyphShape::Outline {
                    program,
                    glyph: id,
                    matrix: font_matrix,
                } if mode != 3 => {
                    placed.outline_key.glyph = id.0;
                    placed.transform = checked(font_matrix.then(transform))?;
                    let outline = self
                        .session
                        .fonts
                        .outline(font, *id, program, self.position)?;
                    if !outline.is_empty() {
                        if mode >= 4 {
                            self.clip_glyph(&outline, placed.transform, scope)?;
                        }
                        if mode != 7 {
                            placed.outline = Some(outline);
                        }
                    }
                }
                GlyphShape::Type3 {
                    procedure,
                    matrix: font_matrix,
                } if mode != 3 && mode != 7 => {
                    placed.transform = checked(font_matrix.then(transform))?;
                    self.type3_glyph(
                        font,
                        *procedure,
                        placed.transform,
                        resources,
                        (glyph.advance, font_matrix.a),
                        scope,
                    )?;
                }
                _ => {}
            }
        }
        text_translate(scope, dx, dy)?;
        Ok(placed)
    }

    /// Adds a glyph outline to the text object's clip.
    fn clip_glyph(&self, outline: &Path, transform: Transform, scope: &mut Scope) -> Result<()> {
        let path = outline.transformed(transform);
        validate(&path)?;
        let limit = self.session.limits.path_segments;
        let clip = &mut text_object(scope)?.clip;
        append(clip, &path, limit)
    }

    /// Emits the glyphs of one string operand: to devices that need text
    /// always, and to others when the string paints a visible glyph.
    fn emit_run(
        &mut self,
        font: &LoadedFont,
        glyphs: &[Glyph],
        text: bool,
        scope: &mut Scope,
    ) -> Result<()> {
        let paints = glyphs.iter().any(|g| g.outline.is_some());
        let hidden = if scope.visible {
            if paints {
                self.open_lazy()?;
            }
            false
        } else {
            true
        };
        let emit = if hidden {
            self.needs().structure && (paints || text)
        } else {
            paints || text
        };
        if !emit {
            return Ok(());
        }
        let mode = pagura_extract::TextRenderingMode::from_low_bits(scope.state.text.mode);
        let paint = self.needs().paint && paints;
        let fill = (paint && mode.fills()).then(|| {
            let composite = scope.state.fill_composite();
            (self.fill_paint(scope), composite)
        });
        let stroke = (paint && mode.strokes()).then(|| {
            let composite = scope.state.stroke_composite();
            (self.stroke_paint(scope), scope.state.pen(), composite)
        });
        let position = self.position;
        let state = |composite| MarkState {
            clip: scope.state.clip.as_ref(),
            composite,
            position,
            hidden,
        };
        self.device.glyphs(&GlyphRun {
            font: &font.info,
            glyphs,
            mode,
            paint: GlyphPaint {
                fill: fill.as_ref().and_then(|(paint, _)| paint.as_ref()),
                fill_state: fill.as_ref().map(|(_, composite)| state(composite)),
                stroke: stroke
                    .as_ref()
                    .and_then(|(paint, pen, _)| paint.as_ref().map(|paint| (paint, pen))),
                stroke_state: stroke.as_ref().map(|(_, _, composite)| state(composite)),
            },
            hidden,
        })
    }
}

/// Checks that every point of a transformed glyph path is finite.
fn validate(path: &Path) -> Result<()> {
    for segment in path.segments() {
        match *segment {
            Segment::MoveTo(p) | Segment::LineTo(p) => finite(p)?,
            Segment::CubicTo { c1, c2, end } => {
                finite(c1)?;
                finite(c2)?;
                finite(end)?;
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

/// What the glyphs of one string share: the text state and CTM, which
/// showing the string does not change.
struct Run {
    size: f64,
    horizontal: f64,
    character: f64,
    word: f64,
    mode: u8,
    ctm: Transform,
    /// Font size, horizontal scaling and rise (9.4.4).
    scale: Transform,
    /// `scale` after the origin of a horizontal glyph, which is the
    /// origin.
    scaled: Transform,
    outlines: bool,
}

impl Run {
    fn new(scope: &Scope, outlines: bool) -> Self {
        let params = &scope.state.text;
        let scale = Transform::new(
            params.size * params.horizontal,
            0.0,
            0.0,
            params.size,
            0.0,
            params.rise,
        );
        Self {
            size: params.size,
            horizontal: params.horizontal,
            character: params.character,
            word: params.word,
            mode: params.mode,
            ctm: scope.state.ctm,
            scale,
            scaled: Transform::translate(0.0, 0.0).then(scale),
            outlines,
        }
    }
}
