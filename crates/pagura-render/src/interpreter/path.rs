//! Path construction, painting and deferred clipping (8.5).

use super::{
    Arc, Clip, Command, Compiler, Error, FillRule, Mark, PathCommand, Point, Result, Value, finite,
    nested, number, point,
};

impl Compiler<'_> {
    pub(super) fn construct(&mut self, word: &[u8], v: &[Value]) -> Result<()> {
        if self.pending_clip.is_some() {
            return Err(Error::syntax("construction follows clipping operator"));
        }
        let p = |i| point(v, i).map(|p| p * self.state.ctm);
        match word {
            b"m" => {
                let point = p(0)?;
                finite(point)?;
                self.path.move_to(point);
            }
            b"l" => {
                let point = p(0)?;
                finite(point)?;
                self.path
                    .line_to(point)
                    .ok_or(Error::syntax("line has no current point"))?;
            }
            b"c" | b"v" | b"y" => {
                let current = self
                    .path
                    .current_point()
                    .ok_or(Error::syntax("curve has no current point"))?;
                let (a, b, end) = match word {
                    b"c" => (p(0)?, p(2)?, p(4)?),
                    b"v" => (current, p(0)?, p(2)?),
                    _ => (p(0)?, p(2)?, p(2)?),
                };
                for point in [a, b, end] {
                    finite(point)?;
                }
                self.path
                    .cubic_to(a, b, end)
                    .ok_or(Error::syntax("curve has no current point"))?;
            }
            b"h" => {
                if self.path.current_point().is_none() {
                    return Err(Error::syntax("close has no current point"));
                }
                self.path.close();
            }
            b"re" => {
                let x = number(v, 0)?;
                let y = number(v, 1)?;
                let w = number(v, 2)?;
                let h = number(v, 3)?;
                let points = [
                    Point::new(x, y),
                    Point::new(x + w, y),
                    Point::new(x + w, y + h),
                    Point::new(x, y + h),
                ]
                .map(|p| p * self.state.ctm);
                for (i, point) in points.into_iter().enumerate() {
                    finite(point)?;
                    if i == 0 {
                        self.path.move_to(point);
                    } else {
                        self.path.line_to(point);
                    }
                }
                self.path.close();
            }
            _ => return Err(Error::syntax("invalid path operator")),
        }
        Ok(())
    }

    pub(super) fn paint(&mut self, word: &[u8]) -> Result<()> {
        if self.path.is_empty() {
            return Err(Error::syntax("painting requires a current path"));
        }
        if matches!(word, b"s" | b"b" | b"b*") {
            self.path.close();
        }
        let mut path = self.path.finish();
        let rule = if matches!(word, b"f*" | b"B*" | b"b*") {
            FillRule::EvenOdd
        } else {
            FillRule::NonZero
        };
        let fill = matches!(word, b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*");
        let stroke = matches!(word, b"S" | b"s" | b"B" | b"B*" | b"b" | b"b*");
        if fill && self.visible {
            self.prepare_fill(&mut path, rule)?;
        }
        if let Some(rule) = self.pending_clip {
            self.prepare_fill(&mut path, rule)?;
        }
        let path = Arc::new(path);
        if self.visible {
            self.marks = self
                .marks
                .checked_add(usize::from(fill) + usize::from(stroke))
                .filter(|&n| n <= self.document.limits().display_commands)
                .ok_or(Error::limit("display list budget exceeded"))?;
        }
        let start = self.list.commands.len();
        if fill && self.visible {
            self.list.commands.push(Command::Path(PathCommand {
                path: Arc::clone(&path),
                position: self.position,
                mark: Mark::Fill(rule),
                clip: self.state.clip.clone(),
                color: self.state.fill.clone(),
                transfer: self.state.transfer.clone(),
                composite: self.state.composite.clone(),
            }));
        }
        if stroke && self.visible {
            self.list.commands.push(Command::Path(PathCommand {
                path: Arc::clone(&path),
                position: self.position,
                mark: Mark::Stroke(
                    self.state.stroke.clone(),
                    self.state.ctm,
                    self.state.stroke_adjustment,
                ),
                clip: self.state.clip.clone(),
                color: self.state.stroke_color.clone(),
                transfer: self.state.transfer.clone(),
                composite: crate::transparency::Composite {
                    alpha: self.state.stroke_alpha,
                    overprint: self.state.stroke_overprint,
                    ..self.state.composite.clone()
                },
            }));
        }
        if fill && stroke && self.visible {
            // The combined operation is a single object for transparency
            // (11.7.4.4); its fill and stroke do not composite with each other.
            let overprint = self.state.composite.overprint
                && self.state.stroke_overprint
                && self.state.composite.alpha == self.state.stroke_alpha;
            let mut commands = self.list.commands.split_off(start);
            if overprint {
                for command in &mut commands {
                    if let Command::Path(p) = command {
                        p.composite.alpha = 1.0;
                        p.composite.blend = crate::blend::BlendMode::Normal;
                        p.composite.mask = None;
                    }
                }
            }
            self.list
                .commands
                .push(Command::Group(crate::transparency::Group {
                    position: self.position,
                    commands,
                    bounds: None,
                    attributes: crate::transparency::Attributes {
                        knockout: !overprint,
                        ..Default::default()
                    },
                    composite: if overprint {
                        self.state.composite.clone()
                    } else {
                        crate::transparency::Composite::opaque()
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
        let mut work = self.fill_work.get();
        let result = path.classify_fill(rule, &self.document.limits(), &mut work);
        self.fill_work.set(work);
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
    fn fill_certification_is_charged_once_and_shared_with_detached_compilers() {
        let document = pagura_document::Document::open(
            pagura_testkit::fixture("zero_area", "zero-width-rectangle.pdf"),
            &pagura_document::OpenOptions::default(),
        )
        .unwrap();
        let compiler = Compiler::new(&document, document.page(0).unwrap()).unwrap();
        let mut builder = pagura_raster::PathBuilder::new();
        builder.rect(3.5, 1.0, 0.0, 6.0);
        let mut path = builder.finish();
        compiler.prepare_fill(&mut path, FillRule::NonZero).unwrap();
        let first = compiler.fill_work.get();
        assert!(first > 0);
        compiler.prepare_fill(&mut path, FillRule::NonZero).unwrap();
        assert_eq!(compiler.fill_work.get(), first);
        let child = compiler.detached();
        let mut other = path.transformed(pagura_raster::Transform::IDENTITY);
        child.prepare_fill(&mut other, FillRule::NonZero).unwrap();
        assert_eq!(compiler.fill_work.get(), first * 2);
        compiler.fill_work.set(document.limits().raster_work);
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
