//! Tiling-cell interpretation in the parent's initial graphics state (8.7.2–8.7.3;
//! 11.6.7). Compile one cell; the compositor places only visible instances.

use super::session::Child;
use super::{Arc, Color, Compiler, Dictionary, Error, Result, Value};
use crate::operators::{Operation, Operator, Target};
use crate::pattern::{
    Definition, DefinitionKind, Kind, PaintType, Pattern, Tiling, TilingDefinition,
};

impl Compiler<'_> {
    /// `SCN`/`scn` in a Pattern space (8.6.8, 8.7.3.1): parses the named
    /// pattern, interprets a tiling cell, and selects the finished pattern.
    pub(super) fn select_pattern(
        &mut self,
        target: Target,
        operands: &[Value],
        resources: &Dictionary,
    ) -> Result<()> {
        let color = self.color_mut(target);
        let Color::Pattern { base, .. } = color else {
            return color.set(operands);
        };
        let base = base.clone();
        let Some((name, components)) = operands.split_last() else {
            return Err(Error::syntax("missing pattern name"));
        };
        let name = name
            .as_name()
            .ok_or(Error::syntax("pattern name expected"))?;
        let context = crate::icc::ColorContext::new(self.document, &self.session.colors);
        let definition = Definition::parse(&context, resources, name.as_bytes(), self.initial_ctm)?;
        let color = match &definition.kind {
            DefinitionKind::Tiling(tiling) if tiling.paint == PaintType::Uncolored => {
                let space = base.ok_or(Error::syntax(
                    "uncoloured pattern requires underlying space",
                ))?;
                let mut color = Color::new(space.as_ref().clone());
                color.set(components)?;
                Some(color)
            }
            _ if !components.is_empty() => {
                // 8.7.3.2, Table 73: a coloured pattern takes only a name;
                // operands that would fit the underlying space are ignored.
                if components.iter().any(|v| v.as_number().is_none())
                    || base
                        .as_ref()
                        .is_none_or(|s| s.components() != components.len())
                {
                    return Err(Error::syntax("invalid unused pattern colour operands"));
                }
                let mut diagnostic = pagura_core::Diagnostic::new(
                    crate::repairs::PATTERN_UNDERLYING,
                    "coloured pattern uses intrinsic colours; extra scn operands ignored (8.7.3.2, Table 73)",
                )
                .at(self.position);
                if let pagura_core::Position::Stream { object, .. } = self.position {
                    diagnostic = diagnostic.in_object(object);
                }
                super::repair(self.document, diagnostic)?;
                None
            }
            _ => None,
        };
        let pattern = self.pattern(definition, color.as_ref())?;
        if let Color::Pattern { selected, .. } = self.color_mut(target) {
            *selected = Some(pattern);
        }
        Ok(())
    }

    fn color_mut(&mut self, target: Target) -> &mut Color {
        match target {
            Target::Stroke => &mut self.state.stroke.color,
            Target::Fill => &mut self.state.fill.color,
        }
    }

    /// The pattern a definition describes. A coloured pattern selected
    /// again in this context reuses its interpreted cell.
    fn pattern(&mut self, definition: Definition, color: Option<&Color>) -> Result<Arc<Pattern>> {
        let reuse = definition.object.filter(|_| color.is_none());
        if let Some(pattern) = reuse.and_then(|id| self.patterns.get(&id)) {
            return Ok(Arc::clone(pattern));
        }
        let kind = match definition.kind {
            DefinitionKind::Shading(mut shading) => {
                self.paint_charge(shading.mesh_bytes(), 0)?;
                // A definition is parsed for this selection, so nothing shares it yet.
                let unique = Arc::get_mut(&mut shading);
                debug_assert!(unique.is_some(), "a fresh shading definition is unshared");
                if let Some(path) = unique.and_then(|s| s.bbox_path.as_mut()) {
                    self.prepare_fill(path, pagura_raster::FillRule::NonZero)?;
                }
                Kind::Shading(shading)
            }
            DefinitionKind::Tiling(tiling) => Kind::Tiling(self.tiling_cell(tiling, color)?),
        };
        let pattern = Arc::new(Pattern {
            kind,
            transform: definition.transform,
        });
        if let Some(id) = reuse {
            self.patterns.insert(id, Arc::clone(&pattern));
        }
        Ok(pattern)
    }

    fn tiling_cell(&self, tiling: TilingDefinition, color: Option<&Color>) -> Result<Tiling> {
        let object = self.document.object(tiling.id)?;
        let d = object
            .dictionary()
            .ok_or(Error::syntax("pattern requires dictionary"))?;
        let resources = self.document.resolve_dictionary(
            d.get(b"Resources")
                .ok_or(Error::syntax("pattern requires Resources"))?,
        )?;
        let mut child = self.child(Child::Pattern {
            tiling: &tiling,
            color,
        })?;
        child
            .sequence(&[object], resources)
            .map_err(|e| e.in_object(tiling.id))?;
        Ok(tiling.tiling(child.commands))
    }

    pub(super) fn pattern_ignores(
        &self,
        operator: Operator,
        operation: Operation<'_>,
        resources: &Dictionary,
    ) -> Result<bool> {
        if !self.stencil_pattern {
            return Ok(false);
        }
        // 8.6.8: restrictions propagate through nested forms. Operand validation
        // precedes this decision; ignored setters cannot change the stencil ink.
        if operator.uses_color() {
            return Ok(true);
        }
        if let Operation::XObject(name) = operation {
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
