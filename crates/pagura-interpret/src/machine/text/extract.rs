//! The extraction record of a shown glyph (9.4.4, 9.10): Unicode, baseline
//! geometry and the selection quad, in default user space.

use super::{Machine, Scope};
use crate::device::Device;
use crate::fonts::LoadedFont;
use pagura_core::{Error, Result};
use pagura_extract::{PositionedGlyph, Quad, TextRenderingMode};
use pagura_raster::{Point, Transform};
use pagura_text::{Glyph, GlyphShape, GlyphText};
use std::sync::Arc;

/// A glyph cell in text space. Unlike a `Rect`, its corners keep the
/// direction of the advance, so the selection quad follows the writing
/// direction (a negative advance gives a reversed cell).
#[derive(Clone, Copy)]
struct Cell {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

impl<D: Device + ?Sized> Machine<'_, '_, D> {
    /// The record of `glyph` shown with text matrix `matrix`; `transform`
    /// maps its text-space cell (origin adjusted) to default user space.
    /// The font index, marked-content path and string number are the text
    /// device's to fill.
    pub(super) fn positioned(
        &mut self,
        font: &LoadedFont,
        glyph: &Glyph,
        matrix: Transform,
        transform: Transform,
        scope: &Scope,
    ) -> Result<PositionedGlyph> {
        let unicode = self.unicode(font, glyph)?;
        let text = &scope.state.text;
        let trm = Transform::new(
            text.size * text.horizontal,
            0.0,
            0.0,
            text.size,
            0.0,
            text.rise,
        )
        .then(matrix)
        .then(scope.state.ctm);
        let origin = trm.apply(Point::default());
        let vertical = glyph.vertical;
        let unit = if vertical.is_some() {
            Point::new(0.0, -1.0)
        } else {
            Point::new(1.0, 0.0)
        };
        let tip = trm.apply(unit);
        let axis = Point::new(tip.x - origin.x, tip.y - origin.y);
        let perp = trm.apply(Point::new(-unit.y, unit.x));
        let em = (perp.x - origin.x).hypot(perp.y - origin.y);
        let advance = vertical.map_or(Point::new(glyph.advance, 0.0), |v| {
            Point::new(0.0, v.advance)
        });
        let advance_end = trm.apply(advance);
        let (cell, cell_transform) = if let GlyphShape::Type3 { matrix, .. } = &glyph.shape {
            let cell = font.bbox.map_or(
                Cell {
                    x0: 0.0,
                    y0: 0.0,
                    x1: 1000.0,
                    y1: 1000.0,
                },
                |b| Cell {
                    x0: b.x0(),
                    y0: b.y0(),
                    x1: b.x1(),
                    y1: b.y1(),
                },
            );
            (cell, matrix.then(transform))
        } else {
            let (bottom, top) = font
                .bbox
                .map_or((0.0, 1.0), |b| (b.y0() / 1000.0, b.y1() / 1000.0));
            let cell = vertical.map_or(
                Cell {
                    x0: 0.0,
                    y0: bottom,
                    x1: glyph.advance,
                    y1: top,
                },
                |v| Cell {
                    x0: 0.0,
                    y0: 0.0,
                    x1: glyph.advance,
                    y1: -v.advance,
                },
            );
            (cell, transform)
        };
        let quad = Quad(
            [
                Point::new(cell.x0, cell.y0),
                Point::new(cell.x1, cell.y0),
                Point::new(cell.x1, cell.y1),
                Point::new(cell.x0, cell.y1),
            ]
            .map(|p| cell_transform.apply(p)),
        );
        if !quad
            .0
            .into_iter()
            .chain([origin, axis, advance_end])
            .all(Point::is_finite)
            || !em.is_finite()
        {
            return Err(Error::syntax("text selection geometry overflow"));
        }
        Ok(PositionedGlyph {
            unicode,
            font: 0,
            size: text.size,
            writing_mode: font.font.writing_mode(),
            quad,
            origin,
            axis,
            em,
            advance_end,
            render_mode: TextRenderingMode::from_low_bits(text.mode),
            hidden: !scope.visible,
            marked_content: Arc::from([]),
            show: 0,
        })
    }

    /// The glyph's Unicode: ToUnicode or encoding names, else the program's
    /// glyph name (9.10.2 permits reader-selected text when those fail).
    fn unicode(&mut self, font: &LoadedFont, glyph: &Glyph) -> Result<Option<GlyphText>> {
        if let Some(text) = &glyph.unicode {
            return Ok(Some(text.clone()));
        }
        let GlyphShape::Outline { program, glyph, .. } = &glyph.shape else {
            return Ok(None);
        };
        let Some(name) = program.glyph_name(*glyph) else {
            return Ok(None);
        };
        self.session.name_work = self
            .session
            .name_work
            .checked_add(name.len())
            .filter(|n| *n <= self.session.limits.text_work)
            .ok_or_else(|| Error::limit("glyph-name Unicode work budget exceeded"))?;
        let base = font
            .info
            .base_name
            .as_ref()
            .map_or(b"".as_slice(), pagura_core::Name::as_bytes);
        Ok(pagura_text::glyph_name_unicode_for_font(name.as_bytes(), base).map(GlyphText::from))
    }
}
