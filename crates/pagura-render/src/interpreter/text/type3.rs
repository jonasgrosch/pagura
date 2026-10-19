//! Type 3 procedure isolation, metrics and colour restrictions (9.6.4).
use super::{Compiler, Dictionary, Error, PdfFont, Result, Transform, Value, number};
use pagura_core::ObjectId;

#[derive(Debug, Clone)]
pub(in crate::interpreter) struct Procedure {
    phase: Phase,
    pub(in crate::interpreter) uncolored: bool,
    inherited_uncolored: bool,
    expected: (f64, f64),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    AwaitMetrics,
    Ready,
}

impl Compiler<'_> {
    pub(in crate::interpreter) fn uncolored_glyph(&self) -> bool {
        self.state
            .text
            .procedure
            .as_ref()
            .is_some_and(|p| p.uncolored)
    }

    pub(in crate::interpreter) fn glyph_operator(&mut self, word: &[u8]) -> Result<bool> {
        let Some(procedure) = &mut self.state.text.procedure else {
            return Ok(false);
        };
        if procedure.phase == Phase::AwaitMetrics && !matches!(word, b"d0" | b"d1") {
            return Err(Error::syntax("Type3 procedure must start with d0 or d1"));
        }
        // 9.6.4, Table 111: colour-related setters in d1 are ignored, after
        // normal operand validation. This also applies in nested form content.
        Ok(procedure.uncolored
            && matches!(
                word,
                b"g" | b"G"
                    | b"rg"
                    | b"RG"
                    | b"k"
                    | b"K"
                    | b"CS"
                    | b"cs"
                    | b"SC"
                    | b"SCN"
                    | b"sc"
                    | b"scn"
                    | b"ri"
                    | b"sh"
            ))
    }
    pub(super) fn glyph_metrics(&mut self, word: &[u8], values: &[Value]) -> Result<()> {
        let procedure = self
            .state
            .text
            .procedure
            .as_mut()
            .ok_or(Error::syntax("glyph metrics outside Type3 procedure"))?;
        if procedure.phase == Phase::Ready {
            return Err(Error::syntax("duplicate Type3 metrics"));
        }
        if number(values, 0)? * procedure.expected.1 != procedure.expected.0
            || number(values, 1)? != 0.0
        {
            super::super::repair(
                self.document,
                pagura_core::Diagnostic::new(
                    super::super::tags::TYPE3_WIDTH,
                    "d0/d1 metrics disagree with Widths or declare a vertical advance; Widths retained",
                )
                .at(self.position)
                .in_object(match self.position {
                    pagura_core::Position::Stream { object, .. } => object,
                    _ => return Err(Error::syntax("Type3 metrics have no stream context")),
                }),
            )?;
        }
        if word == b"d1"
            && (number(values, 2)? > number(values, 4)? || number(values, 3)? > number(values, 5)?)
        {
            return Err(Error::syntax("invalid Type3 glyph bounding box"));
        }
        procedure.phase = Phase::Ready;
        procedure.uncolored = procedure.inherited_uncolored || word == b"d1";
        Ok(())
    }
    pub(super) fn type3_glyph(
        &mut self,
        font: &PdfFont,
        id: ObjectId,
        transform: Transform,
        resources: &Dictionary,
        expected: (f64, f64),
    ) -> Result<()> {
        if self.forms.contains(&id) {
            return Err(Error::cycle("recursive Type3 glyph"));
        }
        if self.forms.len() >= self.document.limits().depth {
            return Err(Error::limit("Type3 nesting budget exceeded"));
        }
        let text_object = self.state.text.object.take();
        let mut child = self.detached();
        // Glyph space does not give an image its device size.
        child.drawn = None;
        self.state.text.object = text_object;
        child.forms.push(id);
        child.state.ctm = transform;
        child.initial_ctm = transform;
        child.state.text.object = None;
        let inherited_uncolored = self.uncolored_glyph();
        child.state.text.procedure = Some(Procedure {
            phase: Phase::AwaitMetrics,
            uncolored: inherited_uncolored,
            inherited_uncolored,
            expected,
        });
        let object = self.document.object(id)?;
        let page_resources = self.state.text.cache.borrow().page_resources.clone();
        child.sequence(
            &[object],
            font.type3_resources()
                .or(page_resources.as_deref())
                .unwrap_or(resources),
        )?;
        if child
            .state
            .text
            .procedure
            .as_ref()
            .is_none_or(|p| p.phase == Phase::AwaitMetrics)
        {
            return Err(Error::syntax("Type3 glyph has no metrics").in_object(id));
        }
        self.operations = child.operations;
        self.marks = child.marks;
        if child.list.commands.is_empty() {
            return Ok(());
        }
        self.paint_charge(0, 1)?;
        self.list
            .commands
            .push(super::super::Command::Group(crate::transparency::Group {
                position: self.position,
                bounds: super::bounds::bounds(&child.list.commands),
                commands: child.list.commands,
                attributes: crate::transparency::Attributes::default(),
                composite: crate::transparency::Composite::opaque(),
            }));
        Ok(())
    }
}
