//! Path construction, painting and deferred clipping (8.5).

use super::{
    Arc, Clip, Command, Compiler, Error, FillRule, Mark, PathCommand, Point, Result, finite, nested,
};
use crate::operators::{Construct, Painting, Target};

impl Compiler<'_> {
    pub(super) fn construct(&mut self, construct: Construct) -> Result<()> {
        if self.pending_clip.is_some() {
            return Err(Error::syntax("construction follows clipping operator"));
        }
        let ctm = self.state.ctm;
        let device = |p: Point| {
            let p = p * ctm;
            finite(p).map(|()| p)
        };
        match construct {
            Construct::MoveTo(point) => {
                self.path.move_to(device(point)?);
            }
            Construct::LineTo(point) => {
                self.path
                    .line_to(device(point)?)
                    .ok_or(Error::syntax("line has no current point"))?;
            }
            Construct::CurveTo([a, b, end]) => {
                self.curve([device(a)?, device(b)?, device(end)?])?;
            }
            Construct::CurveToInitial([b, end]) => {
                let current = self
                    .path
                    .current_point()
                    .ok_or(Error::syntax("curve has no current point"))?;
                self.curve([current, device(b)?, device(end)?])?;
            }
            Construct::CurveToFinal([a, end]) => {
                let end = device(end)?;
                self.curve([device(a)?, end, end])?;
            }
            Construct::Close => {
                if self.path.current_point().is_none() {
                    return Err(Error::syntax("close has no current point"));
                }
                self.path.close();
            }
            Construct::Rectangle {
                origin,
                width,
                height,
            } => {
                let corners = [
                    origin,
                    Point::new(origin.x + width, origin.y),
                    Point::new(origin.x + width, origin.y + height),
                    Point::new(origin.x, origin.y + height),
                ];
                let [first, rest @ ..] = corners;
                self.path.move_to(device(first)?);
                for corner in rest {
                    self.path
                        .line_to(device(corner)?)
                        .ok_or(Error::syntax("line has no current point"))?;
                }
                self.path.close();
            }
        }
        Ok(())
    }

    fn curve(&mut self, [a, b, end]: [Point; 3]) -> Result<()> {
        self.path
            .cubic_to(a, b, end)
            .map(|_| ())
            .ok_or(Error::syntax("curve has no current point"))
    }

    pub(super) fn paint(&mut self, painting: Painting) -> Result<()> {
        if self.path.is_empty() {
            return Err(Error::syntax("painting requires a current path"));
        }
        if painting.close {
            self.path.close();
        }
        let mut path = self.path.finish();
        let rule = painting.fill.unwrap_or(FillRule::NonZero);
        let fill = painting.fill.is_some();
        let stroke = painting.stroke;
        if fill && self.visible {
            self.prepare_fill(&mut path, rule)?;
        }
        if let Some(rule) = self.pending_clip {
            self.prepare_fill(&mut path, rule)?;
        }
        let path = Arc::new(path);
        if self.visible {
            self.session
                .charge_marks(usize::from(fill) + usize::from(stroke))?;
        }
        let start = self.commands.len();
        if fill && self.visible {
            self.commands.push(Command::Path(PathCommand {
                path: Arc::clone(&path),
                position: self.position,
                mark: Mark::Fill(rule),
                clip: self.state.clip.clone(),
                color: self.state.fill.color.clone(),
                transfer: self.state.transfer.clone(),
                composite: self.state.composite(Target::Fill),
            }));
        }
        if stroke && self.visible {
            self.commands.push(Command::Path(PathCommand {
                path: Arc::clone(&path),
                position: self.position,
                mark: Mark::Stroke(
                    self.state.pen.clone(),
                    self.state.ctm,
                    self.state.stroke_adjustment,
                ),
                clip: self.state.clip.clone(),
                color: self.state.stroke.color.clone(),
                transfer: self.state.transfer.clone(),
                composite: self.state.composite(Target::Stroke),
            }));
        }
        if fill && stroke && self.visible {
            // The combined operation is a single object for transparency
            // (11.7.4.4); its fill and stroke do not composite with each other.
            let overprint = self.state.fill.overprint
                && self.state.stroke.overprint
                && self.state.fill.alpha == self.state.stroke.alpha;
            let mut commands = self.commands.split_off(start);
            if overprint {
                for command in &mut commands {
                    if let Command::Path(p) = command {
                        p.composite.alpha = 1.0;
                        p.composite.blend = crate::blend::BlendMode::Normal;
                        p.composite.mask = None;
                    }
                }
            }
            self.commands
                .push(Command::Group(crate::transparency::Group {
                    position: self.position,
                    commands,
                    bounds: None,
                    attributes: crate::transparency::Attributes {
                        knockout: !overprint,
                        ..Default::default()
                    },
                    composite: if overprint {
                        self.state.composite(Target::Fill)
                    } else {
                        crate::transparency::Composite::default()
                    },
                }));
        }
        // 8.5.4: a pending clip takes effect only AFTER the painting operation.
        if let Some(rule) = self.pending_clip.take() {
            self.add_clip(path, rule)?;
        }
        Ok(())
    }

    pub(super) fn prepare_fill(
        &self,
        path: &mut pagura_raster::Path,
        rule: FillRule,
    ) -> Result<()> {
        let mut work = self.session.fill_work.get();
        let result = path.classify_fill(rule, &self.document.limits(), &mut work);
        self.session.fill_work.set(work);
        result
    }

    pub(super) fn add_clip(
        &mut self,
        mut path: Arc<pagura_raster::Path>,
        rule: FillRule,
    ) -> Result<()> {
        if path.fill_region(rule).is_none() {
            self.prepare_fill(Arc::make_mut(&mut path), rule)?;
        }
        self.state.clip_depth = nested(self.state.clip_depth, self.document.limits().depth)?;
        self.state.clip = Some(Arc::new(Clip {
            parent: self.state.clip.clone(),
            path,
            rule,
        }));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_certification_is_charged_once_and_shared_with_child_contexts() {
        let document = pagura_document::Document::open(
            pagura_testkit::fixture("zero_area", "zero-width-rectangle.pdf"),
            &pagura_document::OpenOptions::default(),
        )
        .unwrap();
        let page = document.page(0).unwrap();
        let fonts = std::sync::Mutex::new(super::super::text::cache::FontCache::new(
            document.limits(),
        ));
        let session = super::super::Session::new(&document, &page, None, &fonts, None, None)
            .unwrap();
        let compiler = Compiler::new(&session);
        let mut builder = pagura_raster::PathBuilder::new();
        builder.rect(3.5, 1.0, 0.0, 6.0);
        let mut path = builder.finish();
        compiler.prepare_fill(&mut path, FillRule::NonZero).unwrap();
        let first = session.fill_work.get();
        assert!(first > 0);
        compiler.prepare_fill(&mut path, FillRule::NonZero).unwrap();
        assert_eq!(session.fill_work.get(), first);
        let child = compiler.child(super::super::session::Child::SoftMask).unwrap();
        let mut other = path.transformed(pagura_raster::Transform::IDENTITY);
        child.prepare_fill(&mut other, FillRule::NonZero).unwrap();
        assert_eq!(session.fill_work.get(), first * 2);
        session.fill_work.set(document.limits().raster_work);
        let mut other = path.transformed(pagura_raster::Transform::IDENTITY);
        assert_eq!(
            child
                .prepare_fill(&mut other, FillRule::NonZero)
                .unwrap_err()
                .kind,
            pagura_core::ErrorKind::Limit
        );
    }
}
