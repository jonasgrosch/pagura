//! Showing strings (9.4.3): glyph placement, outlines, the text clip
//! (9.3.6), and one glyph run per string operand for the device.

use super::{Machine, Scope, checked, text_object, text_translate};
use crate::device::{Device, Glyph, GlyphPaint, GlyphRun, MarkState, OutlineKey, TextRun};
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
        let mut strings = std::mem::take(&mut self.session.strings);
        let result = self.show_glyphs(font, bytes, text, &mut strings, resources, scope);
        strings.decoded.clear();
        strings.placed.clear();
        strings.text.clear();
        self.session.strings = strings;
        result
    }

    fn show_glyphs(
        &mut self,
        font: &LoadedFont,
        bytes: &[u8],
        text: bool,
        strings: &mut Strings,
        resources: &Dictionary,
        scope: &mut Scope,
    ) -> Result<()> {
        let unicode = if text { font.unicode.as_deref() } else { None };
        font.font
            .decode_into(bytes, unicode, &mut strings.decoded)
            .map_err(|mut e| {
                e.position = self.position;
                e
            })?;
        let count = strings.decoded.len();
        self.session
            .tokens
            .charge(count)
            .map_err(|_| Error::limit("text glyph work exceeded"))?;
        strings.placed.reserve(count);
        if text {
            strings.text.reserve(count);
        }
        let run = Run::new(scope, self.needs().outlines);
        for glyph in &strings.decoded {
            if text {
                let matrix = text_object(scope)?.matrix;
                let transform = run.transform(glyph, matrix)?;
                strings
                    .text
                    .push(self.positioned(font, glyph, matrix, transform, scope)?);
            }
            let placed = self.glyph(font, glyph, &run, resources, scope)?;
            strings.placed.push(placed);
        }
        self.emit_run(font, strings, text, scope)
    }

    /// Places one glyph: its outline or procedure for paint, its part of
    /// the text clip, and the advance (9.4.4).
    fn glyph(
        &mut self,
        font: &LoadedFont,
        glyph: &pagura_text::Glyph,
        run: &Run,
        resources: &Dictionary,
        scope: &mut Scope,
    ) -> Result<Glyph> {
        let matrix = scope
            .text
            .as_ref()
            .ok_or(Error::syntax("glyph outside text object"))?
            .matrix;
        let transform = run.transform(glyph, matrix)?;
        let spacing = run.character + if glyph.word_space { run.word } else { 0.0 };
        let (dx, dy) = glyph.vertical.map_or(
            ((glyph.advance * run.size + spacing) * run.horizontal, 0.0),
            |v| (0.0, v.advance * run.size + spacing),
        );
        let mut placed = Glyph {
            code: glyph.code,
            outline_key: OutlineKey {
                font: font.info.key,
                glyph: glyph.code,
            },
            transform,
            outline: None,
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

    /// Emits one string operand: its glyphs when it paints one, and its
    /// text when it is page text and the device needs text. Hidden strings
    /// reach only devices that need structure.
    fn emit_run(
        &mut self,
        font: &LoadedFont,
        strings: &mut Strings,
        text: bool,
        scope: &mut Scope,
    ) -> Result<()> {
        let paints = strings.placed.iter().any(|g| g.outline.is_some());
        if !scope.visible && !self.needs().structure {
            return Ok(());
        }
        if paints {
            self.emit_glyphs(font, &strings.placed, scope)?;
        }
        if text {
            self.device.text(TextRun {
                font: &font.info,
                glyphs: &mut strings.text,
            })?;
        }
        Ok(())
    }

    /// Emits the glyphs of a string that paints.
    fn emit_glyphs(
        &mut self,
        font: &LoadedFont,
        glyphs: &[Glyph],
        scope: &mut Scope,
    ) -> Result<()> {
        let hidden = !scope.visible;
        if !hidden {
            self.open_lazy()?;
        }
        let mode = pagura_extract::TextRenderingMode::from_low_bits(scope.state.text.mode);
        let paint = self.needs().paint;
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

/// The buffers of the string being shown, reused from one string to the
/// next: its decoded glyphs, their placements and their records for text.
#[derive(Debug, Default)]
pub(crate) struct Strings {
    decoded: Vec<pagura_text::Glyph>,
    placed: Vec<Glyph>,
    text: Vec<pagura_extract::PositionedGlyph>,
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
    /// Glyph space (before `FontMatrix`) to default user space for `glyph`
    /// at text matrix `matrix`: its origin adjusted for vertical writing.
    fn transform(&self, glyph: &pagura_text::Glyph, matrix: Transform) -> Result<Transform> {
        let scaled = match glyph.vertical {
            Some(v) => Transform::translate(-v.origin.0, -v.origin.1).then(self.scale),
            None => self.scaled,
        };
        checked(scaled.then(matrix).then(self.ctm))
    }

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
