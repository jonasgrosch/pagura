//! Tiling-cell interpretation in the parent's initial graphics state (8.7.2–8.7.3;
//! 11.6.7). Compile one cell; the compositor places only visible instances.

use super::{
    Arc, Color, Compiler, Dictionary, Error, FillRule, PathBuilder, Result, Transform, Value,
};
use crate::pattern::Kind;

impl Compiler<'_> {
    pub(super) fn compile_pattern(&mut self, word: &[u8]) -> Result<()> {
        if !matches!(word, b"SCN" | b"scn") {
            return Ok(());
        }
        let color = if word == b"SCN" {
            &mut self.state.stroke_color
        } else {
            &mut self.state.fill
        };
        let Color::Pattern { selected, .. } = color else {
            return Ok(());
        };
        let Some(mut pattern) = selected.take() else {
            return Ok(());
        };
        let unique = Arc::get_mut(&mut pattern)
            .ok_or(Error::invalid_argument("shared uncompiled pattern"))?;
        if let Kind::Shading(s) = &mut unique.kind {
            self.paint_charge(s.mesh_bytes(), 0)?;
            let shading =
                Arc::get_mut(s).ok_or(Error::invalid_argument("shared uncompiled shading"))?;
            if let Some(path) = &mut shading.bbox_path {
                self.prepare_fill(path, FillRule::NonZero)?;
            }
        }
        if let Kind::Tiling(t) = &mut unique.kind {
            if self.forms.contains(&t.id) {
                return Err(Error::cycle("recursive tiling pattern"));
            }
            if self.forms.len() >= self.document.limits().depth {
                return Err(Error::limit("pattern nesting exceeded"));
            }
            let object = self.document.object(t.id)?;
            let d = object
                .dictionary()
                .ok_or(Error::syntax("pattern requires dictionary"))?;
            let resources = self.document.resolve_dictionary(
                d.get(b"Resources")
                    .ok_or(Error::syntax("pattern requires Resources"))?,
            )?;
            let mut child = self.detached();
            // Pattern space does not give an image its device size.
            child.drawn = None;
            child.forms.push(t.id);
            child.state = self.initial_state.clone();
            child.state.ctm = Transform::IDENTITY;
            child.state.clip = None;
            child.state.clip_depth = 0;
            child.state.text.object = None;
            child.state.text.procedure = None;
            child.state.composite = crate::transparency::Composite::opaque();
            child.state.stroke_alpha = 1.0;
            child.initial_ctm = Transform::IDENTITY;
            child.initial_state = child.state.clone();
            child.stencil_pattern = t.uncolored;
            if let Some(color) = &t.color {
                child.state.fill = color.clone();
                child.state.stroke_color = color.clone();
            }
            let mut bounds = PathBuilder::new();
            bounds.rect(t.bbox.x0, t.bbox.y0, t.bbox.width(), t.bbox.height());
            child.add_clip(Arc::new(bounds.finish()), FillRule::NonZero)?;
            child
                .sequence(&[object], resources)
                .map_err(|e| e.in_object(t.id))?;
            self.operations = child.operations;
            self.marks = child.marks;
            t.commands = child.list.commands;
        }
        let color = if word == b"SCN" {
            &mut self.state.stroke_color
        } else {
            &mut self.state.fill
        };
        if let Color::Pattern { selected, .. } = color {
            *selected = Some(pattern);
        }
        Ok(())
    }

    pub(super) fn pattern_ignores(
        &self,
        word: &[u8],
        v: &[Value],
        resources: &Dictionary,
    ) -> Result<bool> {
        if !self.stencil_pattern {
            return Ok(false);
        }
        // 8.6.8: restrictions propagate through nested forms. Operand validation
        // precedes this decision; ignored setters cannot change the stencil ink.
        if matches!(
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
        ) {
            return Ok(true);
        }
        if word == b"Do" {
            let name = v
                .first()
                .and_then(Value::as_name)
                .ok_or(Error::syntax("XObject name expected"))?;
            let value = super::resource(self.document, resources, b"XObject", name.as_bytes())?;
            let d = self.document.resolve_dictionary(value)?;
            return Ok(d.is_name(b"Subtype", b"Image")
                && d.get(b"ImageMask")
                    .map(|v| self.document.resolve(v))
                    .transpose()?
                    .and_then(Value::as_bool)
                    != Some(true));
        }
        Ok(false)
    }
}
