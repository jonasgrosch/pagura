//! Type 3 glyph procedures (9.6.4): interpreted into ordinary marks in a
//! scope of their own, with their metrics and colour restrictions.

use super::{Machine, Scope};
use crate::device::Device;
use crate::fonts::LoadedFont;
use crate::machine::Text;
use crate::model::{GroupAttributes, GroupCause};
use crate::repairs;
use pagura_content::Op;
use pagura_core::{Diagnostic, Dictionary, Error, ObjectId, Position, Result};
use pagura_document::source::ReadResult;
use pagura_raster::Transform;

/// The state of a glyph procedure: whether its metrics were set, whether
/// `d1` made it uncoloured, and a reversed `d1` box to check at its end.
#[derive(Debug, Clone)]
pub(crate) struct Procedure {
    phase: Phase,
    uncolored: bool,
    inherited_uncolored: bool,
    /// The `Widths` advance and the font matrix's `a`, which `d0`/`d1`
    /// must agree with.
    expected: (f64, f64),
    reversed_bounds: Option<Position>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    AwaitMetrics,
    Ready,
}

impl Scope {
    /// Whether the scope is an uncoloured glyph procedure (`d1`).
    pub(crate) fn uncolored_glyph(&self) -> bool {
        self.procedure.as_ref().is_some_and(|p| p.uncolored)
    }

    /// Applies the rules of glyph procedures to `op`: they start with `d0`
    /// or `d1`, and after `d1` colour operators are ignored (Table 111),
    /// also in nested forms. Returns whether `op` is ignored.
    pub(crate) fn glyph_restriction(&self, op: &Op<'_>) -> Result<bool> {
        let Some(procedure) = &self.procedure else {
            return Ok(false);
        };
        if procedure.phase == Phase::AwaitMetrics
            && !matches!(op, Op::GlyphWidth(_) | Op::GlyphWidthBounds(_))
        {
            return Err(Error::syntax("Type3 procedure must start with d0 or d1"));
        }
        Ok(procedure.uncolored
            && matches!(
                op,
                Op::StrokeColorSpace(_)
                    | Op::FillColorSpace(_)
                    | Op::StrokeColor(_)
                    | Op::FillColor(_)
                    | Op::StrokeColorN(_)
                    | Op::FillColorN(_)
                    | Op::StrokeGray(_)
                    | Op::FillGray(_)
                    | Op::StrokeRgb(_)
                    | Op::FillRgb(_)
                    | Op::StrokeCmyk(_)
                    | Op::FillCmyk(_)
                    | Op::RenderingIntent(_)
                    | Op::Shading(_)
            ))
    }
}

impl<D: Device + ?Sized> Machine<'_, '_, D> {
    /// `d0` and `d1`.
    pub(super) fn glyph_metrics(&mut self, op: &Op<'_>, scope: &mut Scope) -> Result<()> {
        let position = self.position;
        let (width, bounds) = match *op {
            Op::GlyphWidth(width) => (width, None),
            Op::GlyphWidthBounds([wx, wy, llx, lly, urx, ury]) => {
                ([wx, wy], Some([llx, lly, urx, ury]))
            }
            _ => return Err(Error::syntax("invalid glyph metrics operator")),
        };
        let procedure = scope
            .procedure
            .as_mut()
            .ok_or(Error::syntax("glyph metrics outside Type3 procedure"))?;
        if procedure.phase == Phase::Ready {
            return Err(Error::syntax("duplicate Type3 metrics"));
        }
        if width[0] * procedure.expected.1 != procedure.expected.0 || width[1] != 0.0 {
            let Position::Stream { object, .. } = position else {
                return Err(Error::syntax("Type3 metrics have no stream context"));
            };
            self.session.document.repair(
                Diagnostic::new(
                    repairs::TYPE3_WIDTH,
                    "d0/d1 metrics disagree with Widths or declare a vertical advance; Widths retained",
                )
                .at(position)
                .in_object(object),
            )?;
        }
        if let Some([llx, lly, urx, ury]) = bounds
            && (llx > urx || lly > ury)
        {
            // Some writers emit reversed d1 bounds for empty glyphs; the
            // procedure must prove it paints nothing.
            procedure.reversed_bounds = Some(position);
        }
        procedure.phase = Phase::Ready;
        procedure.uncolored = procedure.inherited_uncolored || bounds.is_some();
        Ok(())
    }

    /// Interprets the glyph procedure `id` of `font` under `transform`
    /// (glyph space to default user space). Its marks form a group of their
    /// own, which opens at the first visible mark.
    pub(super) fn type3_glyph(
        &mut self,
        font: &LoadedFont,
        id: ObjectId,
        transform: Transform,
        resources: &Dictionary,
        expected: (f64, f64),
        scope: &Scope,
    ) -> ReadResult<()> {
        if scope.forms.contains(&id) {
            return Err(Error::cycle("recursive Type3 glyph").into());
        }
        if scope.forms.len() >= self.session.limits.depth {
            return Err(Error::limit("Type3 nesting budget exceeded").into());
        }
        let document = self.document();
        let mut child = scope.child(Text::Paint);
        child.forms.push(id);
        child.state.set_ctm(transform);
        child.initial_ctm = transform;
        let inherited_uncolored = scope.uncolored_glyph();
        child.procedure = Some(Procedure {
            phase: Phase::AwaitMetrics,
            uncolored: inherited_uncolored,
            inherited_uncolored,
            expected,
            reversed_bounds: None,
        });
        let object = document.object(id)?;
        let page_resources = self.session.page_resources;
        let resources = font
            .font
            .type3_resources()
            .or(page_resources)
            .unwrap_or(resources);
        let content = self.stream_content(object)?;
        self.push_lazy(GroupCause::Type3Glyph, GroupAttributes::default(), scope);
        let result = self.run(&content, resources, &mut child);
        let opened = self.pop_lazy();
        result?;
        let painted = opened?;
        let procedure = child.procedure.as_ref();
        if procedure.is_none_or(|p| p.phase == Phase::AwaitMetrics) {
            return Err(Error::syntax("Type3 glyph has no metrics")
                .in_object(id)
                .into());
        }
        if let Some(position) = procedure.and_then(|p| p.reversed_bounds) {
            if painted {
                return Err(Error::syntax("invalid Type3 glyph bounding box")
                    .at(position)
                    .in_object(id)
                    .into());
            }
            document.repair(
                Diagnostic::new(
                    pagura_text::repairs::TYPE3_EMPTY_BOUNDS,
                    "reversed d1 bounds ignored for an empty Type3 glyph; Widths retained",
                )
                .at(position)
                .in_object(id),
            )?;
        }
        Ok(())
    }
}
