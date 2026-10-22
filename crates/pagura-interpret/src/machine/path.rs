//! Path construction, painting and deferred clipping (8.5).

use super::emit::GroupStart;
use super::{Machine, Scope, nested};
use crate::device::Device;
use crate::model::{Clip, FillStroke, GroupAttributes, GroupCause};
use crate::repairs;
use pagura_content::{Op, Painting, Rule};
use pagura_core::{Error, Result};
use pagura_raster::{FillRule, Path, Point};
use std::sync::Arc;

/// The raster fill rule of a content rule.
pub(crate) fn fill_rule(rule: Rule) -> FillRule {
    match rule {
        Rule::NonZero => FillRule::NonZero,
        Rule::EvenOdd => FillRule::EvenOdd,
    }
}

/// Checks that a constructed point is finite.
pub(crate) fn finite(point: Point) -> Result<()> {
    if !point.x.is_finite() || !point.y.is_finite() {
        return Err(Error::syntax("path arithmetic overflow"));
    }
    Ok(())
}

/// `W` and `W*`: the clip applies after the next painting operator (8.5.4).
pub(super) fn clip_operator(rule: Rule, scope: &mut Scope) -> Result<()> {
    if scope.pending_clip.is_some() {
        return Err(Error::syntax("multiple clipping operators for one path"));
    }
    scope.pending_clip = Some(fill_rule(rule));
    Ok(())
}

impl<D: Device + ?Sized> Machine<'_, '_, D> {
    /// A path construction operator; construction after `W` is a repair.
    pub(super) fn construct(&mut self, op: &Op<'_>, scope: &mut Scope) -> Result<()> {
        if scope.pending_clip.is_some() {
            self.repair(
                repairs::CLIP_ORDER,
                "construction follows clipping operator",
            )?;
        }
        construct(op, scope)
    }

    /// A path-painting operator; painting without a path is a repair.
    pub(super) fn paint_operator(&mut self, painting: Painting, scope: &mut Scope) -> Result<()> {
        if scope.path.is_empty() {
            self.repair(repairs::EMPTY_PATH, "painting requires a current path")?;
            scope.pending_clip = None;
            return Ok(());
        }
        if painting.closes() {
            scope.path.close();
        }
        let path = Arc::new(scope.path.finish());
        self.paint_path(&path, painting, scope)?;
        // 8.5.4: a pending clip takes effect only after painting.
        if let Some(rule) = scope.pending_clip.take() {
            add_clip(scope, path, rule, self.session.limits.depth)?;
        }
        Ok(())
    }

    /// Fills and strokes `path` as `painting` asks. A fill and stroke of
    /// one operator are one object for transparency (11.7.4.4): a knockout
    /// group, or under equal overprinting alphas a group carrying the
    /// shared state.
    pub(crate) fn paint_path(
        &mut self,
        path: &Arc<Path>,
        painting: Painting,
        scope: &mut Scope,
    ) -> Result<()> {
        let fill = painting.fill().map(fill_rule);
        let stroke = painting.strokes();
        match (fill, stroke) {
            (Some(rule), true) => {
                if !scope.visible && !self.needs().structure {
                    return Ok(());
                }
                let fill = scope.state.fill_composite();
                let stroke = scope.state.stroke_composite();
                let states = FillStroke::new(&fill, &stroke, &self.session.opaque);
                let start = GroupStart {
                    cause: GroupCause::FillStroke,
                    attributes: GroupAttributes {
                        knockout: states.knockout,
                        ..GroupAttributes::default()
                    },
                    bbox: None,
                    view: None,
                    composite: &states.group,
                };
                let visible = scope.visible;
                if visible {
                    self.begin_group(&start, scope)?;
                }
                self.emit_fill(scope, path, rule, Some(Arc::clone(&states.fill)))?;
                self.emit_stroke(scope, path, Some(Arc::clone(&states.stroke)))?;
                if visible {
                    self.device.end_group()?;
                }
                Ok(())
            }
            (Some(rule), false) => self.emit_fill(scope, path, rule, None),
            (None, true) => self.emit_stroke(scope, path, None),
            (None, false) => Ok(()),
        }
    }
}

fn construct(op: &Op<'_>, scope: &mut Scope) -> Result<()> {
    let ctm = scope.state.ctm;
    let p = |x: f64, y: f64| Point::new(x, y) * ctm;
    let path = &mut scope.path;
    match *op {
        Op::MoveTo([x, y]) => {
            let point = p(x, y);
            finite(point)?;
            path.move_to(point);
        }
        Op::LineTo([x, y]) => {
            let point = p(x, y);
            finite(point)?;
            path.line_to(point)
                .map_err(|_| Error::syntax("line has no current point"))?;
        }
        Op::CurveTo(_) | Op::CurveToInitial(_) | Op::CurveToFinal(_) => {
            let current = path
                .current_point()
                .ok_or(Error::syntax("curve has no current point"))?;
            let (a, b, end) = match *op {
                Op::CurveTo(c) => (p(c[0], c[1]), p(c[2], c[3]), p(c[4], c[5])),
                Op::CurveToInitial(c) => (current, p(c[0], c[1]), p(c[2], c[3])),
                Op::CurveToFinal(c) => (p(c[0], c[1]), p(c[2], c[3]), p(c[2], c[3])),
                _ => return Err(Error::syntax("invalid path operator")),
            };
            for point in [a, b, end] {
                finite(point)?;
            }
            path.cubic_to(a, b, end)
                .map_err(|_| Error::syntax("curve has no current point"))?;
        }
        Op::ClosePath => {
            if path.current_point().is_none() {
                return Err(Error::syntax("close has no current point"));
            }
            path.close();
        }
        Op::Rectangle([x, y, w, h]) => {
            let points = [
                Point::new(x, y),
                Point::new(x + w, y),
                Point::new(x + w, y + h),
                Point::new(x, y + h),
            ]
            .map(|point| point * ctm);
            for point in points {
                finite(point)?;
            }
            path.polygon(points);
        }
        _ => return Err(Error::syntax("invalid path operator")),
    }
    Ok(())
}

/// Intersects the clip of `scope` with `path` (8.5.4).
pub(crate) fn add_clip(
    scope: &mut Scope,
    path: Arc<Path>,
    rule: FillRule,
    depth: usize,
) -> Result<()> {
    let state = &mut scope.state;
    state.clip_depth = nested(state.clip_depth, depth)?;
    state.clip = Some(Arc::new(Clip::new(state.clip.take(), path, rule)));
    Ok(())
}
