//! Shading invocation leaves the current path and colour unchanged (8.7.4.2).

use super::{Arc, Clip, Command, Compiler, Dictionary, FillRule, Result, nested, resource};
use crate::display::ShadingCommand;

impl Compiler<'_> {
    pub(super) fn shading(
        &mut self,
        name: &pagura_core::Name,
        resources: &Dictionary,
    ) -> Result<()> {
        let value = resource(self.document, resources, b"Shading", name.as_bytes())?;
        let context = self.session.colors();
        let mut shading = crate::shading::Shading::parse(&context, resources, value)?;
        if let Some(path) = &mut shading.bbox_path {
            self.prepare_fill(path, FillRule::NonZero)?;
        }
        let shading = Arc::new(shading);
        self.session.retain(shading.mesh_bytes())?;
        if !self.visible {
            return Ok(());
        }
        self.session.charge_marks(1)?;
        let mut clip = self.state.clip.clone();
        if let Some(b) = shading.bbox {
            // The BBox clips like a clipping operator (8.7.4.2), so it
            // nests within the same depth budget.
            nested(self.state.clip_depth, self.document.limits().depth)?;
            let mut path = crate::device::rectangle(b).transformed(self.state.ctm);
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

#[cfg(test)]
mod tests {
    use crate::compile_page;
    use pagura_core::{ErrorKind, Limits};
    use pagura_document::{Document, OpenOptions};

    #[test]
    fn a_shading_bbox_counts_against_the_clip_depth() {
        let open = |depth| {
            Document::open(
                pagura_testkit::fixture("content", "shading-bbox-clip-depth.pdf"),
                &OpenOptions {
                    limits: Limits {
                        depth,
                        ..Limits::default()
                    },
                    ..OpenOptions::default()
                },
            )
            .unwrap()
        };
        let document = open(Limits::default().depth);
        let error = compile_page(&document, document.page(0).unwrap()).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit);
        let document = open(Limits::default().depth + 1);
        assert!(compile_page(&document, document.page(0).unwrap()).is_ok());
    }
}
