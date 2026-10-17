//! Path construction, painting and deferred clipping (8.5).

use super::{
    Arc, Clip, Command, Compiler, Error, FillRule, Mark, Point, Result, Value, finite, nested,
    number, point,
};

impl Compiler<'_> {
    pub(super) fn construct(&mut self, word: &[u8], v: &[Value]) -> Result<()> {
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
        if matches!(word, b"s" | b"b" | b"b*") {
            self.path.close();
        }
        let path = Arc::new(self.path.finish());
        let rule = if matches!(word, b"f*" | b"B*" | b"b*") {
            FillRule::EvenOdd
        } else {
            FillRule::NonZero
        };
        let fill = matches!(word, b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*");
        let stroke = matches!(word, b"S" | b"s" | b"B" | b"B*" | b"b" | b"b*");
        if self
            .list
            .commands
            .len()
            .checked_add(usize::from(fill) + usize::from(stroke))
            .is_none_or(|n| n > self.document.limits().display_commands)
        {
            return Err(Error::limit("display list budget exceeded"));
        }
        if fill && self.visible {
            self.list.commands.push(Command {
                path: Arc::clone(&path),
                mark: Mark::Fill(rule),
                clip: self.state.clip.clone(),
                color: self.state.fill.clone(),
                transfer: self.state.transfer.clone(),
            });
        }
        if stroke && self.visible {
            self.list.commands.push(Command {
                path: Arc::clone(&path),
                mark: Mark::Stroke(
                    self.state.stroke.clone(),
                    self.state.ctm,
                    self.state.stroke_adjustment,
                ),
                clip: self.state.clip.clone(),
                color: self.state.stroke_color.clone(),
                transfer: self.state.transfer.clone(),
            });
        }
        // 8.5.4: a pending clip takes effect only AFTER the painting operation.
        if let Some(rule) = self.pending_clip.take() {
            self.add_clip(path, rule)?;
        }
        Ok(())
    }

    pub(super) fn add_clip(
        &mut self,
        path: Arc<pagura_raster::Path>,
        rule: FillRule,
    ) -> Result<()> {
        self.state.clip_depth = nested(self.state.clip_depth, self.document.limits().depth)?;
        self.state.clip = Some(Arc::new(Clip {
            parent: self.state.clip.clone(),
            path,
            rule,
        }));
        Ok(())
    }
}
