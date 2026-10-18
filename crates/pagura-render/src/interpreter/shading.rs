//! Shading invocation leaves the current path and colour unchanged (8.7.4.2).

use super::{
    Arc, Clip, Command, Compiler, Dictionary, Error, FillRule, PathBuilder, Result, Value, resource,
};
use crate::display::ShadingCommand;

impl Compiler<'_> {
    pub(super) fn shading(&mut self, values: &[Value], resources: &Dictionary) -> Result<()> {
        let name = values
            .first()
            .and_then(Value::as_name)
            .ok_or(Error::syntax("shading name expected"))?;
        let value = resource(self.document, resources, b"Shading", name.as_bytes())?;
        let context = crate::icc::ColorContext::new(self.document, &self.compilation.colors);
        let mut shading = crate::shading::Shading::parse(&context, resources, value)?;
        if let Some(path) = &mut shading.bbox_path {
            self.prepare_fill(path, FillRule::NonZero)?;
        }
        let shading = Arc::new(shading);
        self.paint_charge(shading.mesh_bytes(), 0)?;
        if !self.visible {
            return Ok(());
        }
        self.marks = self
            .marks
            .checked_add(1)
            .filter(|&n| n <= self.document.limits().display_commands)
            .ok_or(Error::limit("display list budget exceeded"))?;
        let mut clip = self.state.clip.clone();
        if let Some(b) = shading.bbox {
            let mut path = PathBuilder::new();
            path.rect(b.x0(), b.y0(), b.width(), b.height());
            let mut path = path.finish().transformed(self.state.ctm);
            self.prepare_fill(&mut path, FillRule::NonZero)?;
            clip = Some(Arc::new(Clip {
                parent: clip,
                path: Arc::new(path),
                rule: FillRule::NonZero,
            }));
        }
        self.list.commands.push(Command::Shading(ShadingCommand {
            shading,
            position: self.position,
            clip,
            transform: self.state.ctm,
            composite: self.state.composite.clone(),
            transfer: self.state.transfer.clone(),
            background: false,
        }));
        Ok(())
    }
}
