//! Shading invocation leaves the current path and colour unchanged (8.7.4.2).

use super::{Arc, Clip, Command, Compiler, Dictionary, FillRule, PathBuilder, Result, resource};
use crate::display::ShadingCommand;

impl Compiler<'_> {
    pub(super) fn shading(
        &mut self,
        name: &pagura_core::Name,
        resources: &Dictionary,
    ) -> Result<()> {
        let value = resource(self.document, resources, b"Shading", name.as_bytes())?;
        let context = crate::icc::ColorContext::new(self.document, &self.session.colors);
        let mut shading = crate::shading::Shading::parse(&context, resources, value)?;
        if let Some(path) = &mut shading.bbox_path {
            self.prepare_fill(path, FillRule::NonZero)?;
        }
        let shading = Arc::new(shading);
        self.paint_charge(shading.mesh_bytes(), 0)?;
        if !self.visible {
            return Ok(());
        }
        self.session.charge_marks(1)?;
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
        self.commands.push(Command::Shading(ShadingCommand {
            shading,
            position: self.position,
            clip,
            transform: self.state.ctm,
            composite: self.state.composite(crate::operators::Target::Fill),
            transfer: self.state.transfer.clone(),
            background: false,
        }));
        Ok(())
    }
}
