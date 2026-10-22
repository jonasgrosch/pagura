//! Glyph placements (9.3.8): each a mark of its own, with its source's
//! hull and edges found once per text-showing operator, gathered into its
//! run's item where it can be (`runs`).

use super::{Colour, Geometry, Shape, Spec};
use crate::plan::{Cover, Mark, area, intersect};
use crate::prepare::planner::clips::Clipping;
use crate::prepare::planner::runs::{Memo, Placed};
use crate::prepare::planner::{Planner, Scope};
use crate::prepare::{Budget, bounds, index};
use pagura_core::{Error, Position, Result};
use pagura_interpret::GlyphsNode;
use pagura_raster::Transform;

impl Planner<'_> {
    /// A glyph run: one item for its placements where they share a solid
    /// paint and need no transfer selection (`runs`), one per placement
    /// otherwise. Either way each placement is prepared, culled and charged
    /// as a mark of its own, so errors come in the same order.
    pub(crate) fn glyphs(
        &mut self,
        scope: &Scope,
        node: &GlyphsNode,
        budget: &mut Budget,
    ) -> Result<()> {
        let gather = node.placements.len() > 1 && self.gathers(scope, node);
        let outer = std::mem::replace(&mut self.gathering, gather.then_some(scope.group));
        let outer_memo = self.memo.take();
        let outer_edges = std::mem::replace(&mut self.glyph_edges, vec![None; node.sources.len()]);
        let placed = self.placements(scope, node, budget);
        self.gathering = outer;
        self.memo = outer_memo;
        self.glyph_edges = outer_edges;
        placed?;
        if gather {
            let position = self.run.as_ref().map(|open| open.position);
            if let Err(error) = self.flush(budget) {
                return Err(self.fail(error, position.unwrap_or_default()));
            }
        }
        Ok(())
    }

    fn placements(&mut self, scope: &Scope, node: &GlyphsNode, budget: &mut Budget) -> Result<()> {
        let t = scope.transform;
        let device = Transform::new(t.a, t.b, t.c, t.d, 0.0, 0.0);
        // Each source's device linear map and hull relative to the origin,
        // once.
        let mut sources: Vec<Option<([f64; 4], Option<bounds::Hull>)>> =
            vec![None; node.sources.len()];
        let mut positions = Positions::new(node);
        for (i, placement) in node.placements.iter().enumerate() {
            let position = positions.at(i);
            let source_index = index(placement.source);
            let Some(source) = node.sources.get(source_index) else {
                self.serial += 1;
                return Err(self.fail(
                    Error::invalid_argument("glyph source out of range"),
                    position,
                ));
            };
            let cached = sources.get(source_index).copied().flatten();
            let (linear, relative) = if let Some(cached) = cached {
                cached
            } else {
                let l = source.linear;
                let linear = Transform::new(l.a, l.b, l.c, l.d, 0.0, 0.0).then(device);
                let mut edges = self.glyph_edges.get(source_index).copied().flatten();
                let hull = self.shapes.glyph_hull((source, &mut edges), linear);
                if let Some(slot) = self.glyph_edges.get_mut(source_index) {
                    *slot = edges;
                }
                let found = ([linear.a, linear.b, linear.c, linear.d], hull);
                if let Some(slot) = sources.get_mut(source_index) {
                    *slot = Some(found);
                }
                found
            };
            let origin = t.apply(placement.origin);
            let hull = relative.map(|[x0, y0, x1, y1]| {
                bounds::expand(
                    [x0 + origin.x, y0 + origin.y, x1 + origin.x, y1 + origin.y],
                    1.0,
                )
            });
            let spec = Spec {
                info: &node.mark,
                position,
                hull,
                geometry: Geometry::Glyph(source, linear, origin, source_index),
                colour: Colour::Paint(node.paint),
            };
            if let Some(memo) = self.memo.filter(|_| self.gathering == Some(scope.group)) {
                self.serial += 1;
                let placed = self.placement(scope, (&memo, &spec), budget);
                placed.map_err(|error| self.fail(error, position))?;
                continue;
            }
            self.mark(scope, &spec, budget)?;
        }
        Ok(())
    }

    /// A glyph placement of the run being gathered, with what its run's
    /// first prepared placement found: culled, its shape prepared within
    /// the clip state, its mask use and work charged, then gathered.
    fn placement(
        &mut self,
        scope: &Scope,
        (memo, spec): (&Memo, &Spec<'_>),
        budget: &mut Budget,
    ) -> Result<()> {
        let region = self.area.region;
        let culling = intersect(
            intersect(bounds::device(spec.hull, region), memo.clip_bounds),
            scope.bounds,
        );
        if culling.is_empty() {
            return Ok(());
        }
        let (clip, clip_rect) = match memo.clipping {
            Clipping::Empty => return Ok(()),
            Clipping::All => (None, region),
            Clipping::Shape(prepared) => (Some(prepared.key), prepared.bounds),
        };
        self.shapes.set_clip(clip);
        let shape = self.shape(scope, &spec.geometry, budget);
        self.shapes.set_clip(None);
        let Shape::Prepared(prepared) = shape? else {
            return Ok(());
        };
        let bounds = intersect(
            intersect(prepared.bounds, clip_rect),
            intersect(scope.bounds, culling),
        );
        if bounds.is_empty() {
            return Ok(());
        }
        budget.bytes.charge(prepared.bytes)?;
        self.use_mask(memo.mask, bounds, budget)?;
        budget.work.charge(prepared.work)?;
        budget.work.charge(area(bounds))?;
        let mark = Mark {
            cover: Cover {
                shape: Some(prepared.key),
                clip,
            },
            ..memo.mark
        };
        let placed = Placed {
            shape: prepared.key,
            bounds,
            work: prepared.work,
            serial: self.serial,
            position: spec.position,
        };
        self.gather(scope.group, mark, placed, budget)
    }
}

/// The positions of a glyph node's placements in show order: its
/// `(first, position)` pairs walked once, as [`GlyphsNode::position`] finds
/// each.
struct Positions<'a> {
    node: &'a GlyphsNode,
    next: usize,
    current: Position,
}

impl<'a> Positions<'a> {
    fn new(node: &'a GlyphsNode) -> Self {
        Self {
            node,
            next: 0,
            current: node.mark.position,
        }
    }

    /// The position of placement `index`; indices come in increasing order.
    fn at(&mut self, index: usize) -> Position {
        while let Some(&(first, position)) = self.node.positions.get(self.next)
            && pagura_core::convert::u32_to_usize(first) <= index
        {
            self.current = position;
            self.next += 1;
        }
        self.current
    }
}
