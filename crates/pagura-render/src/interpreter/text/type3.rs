//! Type 3 procedure isolation, metrics and colour restrictions (9.6.4).
use super::super::session::Child;
use super::{Compiler, Error, PdfFont, Result, Transform};
use crate::operators::{GlyphMetrics, Operator};
use pagura_core::ObjectId;

#[derive(Debug, Clone)]
pub(in crate::interpreter) struct Procedure {
    phase: Phase,
    pub(in crate::interpreter) uncolored: bool,
    inherited_uncolored: bool,
    expected: Advance,
}
/// The glyph width the font's `Widths` require: `wx` times the font
/// matrix's horizontal scale must equal `advance` (9.6.4).
#[derive(Debug, Clone, Copy)]
pub(in crate::interpreter) struct Advance {
    pub(in crate::interpreter) advance: f64,
    pub(in crate::interpreter) scale: f64,
}
impl Procedure {
    /// A procedure that has not set its metrics yet, uncoloured if it runs
    /// inside an uncoloured glyph.
    pub(in crate::interpreter) fn new(inherited_uncolored: bool, expected: Advance) -> Self {
        Self {
            phase: Phase::AwaitMetrics,
            uncolored: inherited_uncolored,
            inherited_uncolored,
            expected,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    AwaitMetrics,
    Ready,
}

impl Compiler<'_> {
    pub(in crate::interpreter) fn uncolored_glyph(&self) -> bool {
        self.procedure.as_ref().is_some_and(|p| p.uncolored)
    }

    /// Whether an uncoloured glyph ignores `operator` (9.6.4, Table 111),
    /// after normal operand validation; this also applies in nested form
    /// content. A procedure must start with `d0` or `d1`.
    pub(in crate::interpreter) fn glyph_ignores(&mut self, operator: Operator) -> Result<bool> {
        let Some(procedure) = &mut self.procedure else {
            return Ok(false);
        };
        if procedure.phase == Phase::AwaitMetrics
            && !matches!(operator, Operator::GlyphWidth | Operator::GlyphBox)
        {
            return Err(Error::syntax("Type3 procedure must start with d0 or d1"));
        }
        Ok(procedure.uncolored && operator.uses_color())
    }
    pub(in crate::interpreter) fn glyph_metrics(&mut self, metrics: GlyphMetrics) -> Result<()> {
        let procedure = self
            .procedure
            .as_mut()
            .ok_or(Error::syntax("glyph metrics outside Type3 procedure"))?;
        if procedure.phase == Phase::Ready {
            return Err(Error::syntax("duplicate Type3 metrics"));
        }
        let ([wx, wy], uncolored) = match metrics {
            GlyphMetrics::Width(width) => (width, false),
            GlyphMetrics::Box {
                width,
                bbox: [x0, y0, x1, y1],
            } => {
                if x0 > x1 || y0 > y1 {
                    return Err(Error::syntax("invalid Type3 glyph bounding box"));
                }
                (width, true)
            }
        };
        if wx * procedure.expected.scale != procedure.expected.advance || wy != 0.0 {
            super::super::repair(
                self.document,
                pagura_core::Diagnostic::new(
                    crate::repairs::TYPE3_WIDTH,
                    "d0/d1 metrics disagree with Widths or declare a vertical advance; Widths retained",
                )
                .at(self.position)
                .in_object(match self.position {
                    pagura_core::Position::Stream { object, .. } => object,
                    _ => return Err(Error::syntax("Type3 metrics have no stream context")),
                }),
            )?;
        }
        procedure.phase = Phase::Ready;
        procedure.uncolored = procedure.inherited_uncolored || uncolored;
        Ok(())
    }
    /// Interprets a Type 3 glyph procedure; its resources fall back to the
    /// page's (9.6.5, Table 110).
    pub(super) fn type3_glyph(
        &mut self,
        font: &PdfFont,
        id: ObjectId,
        transform: Transform,
        expected: Advance,
    ) -> Result<()> {
        let mut child = self.child(Child::Glyph {
            id,
            transform,
            expected,
        })?;
        let object = self.document.object(id)?;
        child.sequence(
            &[object],
            font.type3_resources()
                .unwrap_or(self.session.page.resources()),
        )?;
        if child
            .procedure
            .as_ref()
            .is_none_or(|p| p.phase == Phase::AwaitMetrics)
        {
            return Err(Error::syntax("Type3 glyph has no metrics").in_object(id));
        }
        if child.commands.is_empty() {
            return Ok(());
        }
        self.paint_charge(0, 1)?;
        self.commands
            .push(super::super::Command::Group(crate::transparency::Group {
                position: self.position,
                bounds: super::bounds::bounds(&child.commands),
                commands: child.commands,
                attributes: crate::transparency::Attributes::default(),
                composite: crate::transparency::Composite::default(),
            }));
        Ok(())
    }
}
