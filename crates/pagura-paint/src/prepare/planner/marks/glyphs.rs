//! Glyph placements (9.3.8): each a mark of its own, with its source's
//! hull and edges found once per text-showing operator, gathered into its
//! run's item where it can be (`runs`).

use super::{Colour, Geometry, Shape, Spec};
use crate::plan::{Cover, Mark, area, intersect};
use crate::prepare::planner::clips::Clipping;
use crate::prepare::planner::runs::{Memo, Placed};
use crate::prepare::planner::{Planner, Scope};
use crate::prepare::{Budget, bounds, index};
use pagura_core::{Error, Meter, Position, Result};
use pagura_interpret::GlyphsNode;
use pagura_raster::Transform;
use std::cell::Cell;

/// What a node's placements share for one source, prepared on first use.
struct Source {
    linear: [f64; 4],
    relative: Option<bounds::Hull>,
    edges: Cell<Option<u32>>,
}

/// Makes the one source lookup array, charged before allocation. Empty
/// nodes need none; callers retain the charge until the array is dropped.
fn sources(slots: &mut Vec<Option<Source>>, count: usize, bytes: &mut Meter) -> Result<()> {
    bytes.reserve_capacity(slots, count)?;
    slots.resize_with(count, || None);
    Ok(())
}

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
        let mut sources = Vec::new();
        let placed = self.placements(scope, node, &mut sources, budget);
        let released = sources.capacity() * size_of::<Option<Source>>();
        drop(sources);
        budget.bytes.refund(released);
        self.gathering = outer;
        self.memo = outer_memo;
        placed?;
        if gather {
            let position = self.run.as_ref().map(|open| open.position);
            if let Err(error) = self.flush(budget) {
                return Err(self.fail(error, position.unwrap_or_default()));
            }
        }
        Ok(())
    }

    fn placements(
        &mut self,
        scope: &Scope,
        node: &GlyphsNode,
        cached: &mut Vec<Option<Source>>,
        budget: &mut Budget,
    ) -> Result<()> {
        let t = scope.transform;
        let device = Transform::new(t.a, t.b, t.c, t.d, 0.0, 0.0);
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
            if cached.is_empty()
                && let Err(error) = sources(cached, node.sources.len(), &mut budget.bytes)
            {
                self.serial += 1;
                return Err(self.fail(error, position));
            }
            let slot = cached
                .get_mut(source_index)
                .ok_or(Error::invalid_argument("glyph source out of range"))?;
            let found = slot.get_or_insert_with(|| {
                let l = source.linear;
                let linear = Transform::new(l.a, l.b, l.c, l.d, 0.0, 0.0).then(device);
                let mut edges = None;
                let hull = self.shapes.glyph_hull((source, &mut edges), linear);
                Source {
                    linear: [linear.a, linear.b, linear.c, linear.d],
                    relative: hull,
                    edges: Cell::new(edges),
                }
            });
            let origin = t.apply(placement.origin);
            let hull = found.relative.map(|[x0, y0, x1, y1]| {
                bounds::expand(
                    [x0 + origin.x, y0 + origin.y, x1 + origin.x, y1 + origin.y],
                    1.0,
                )
            });
            let spec = Spec {
                info: &node.mark,
                position,
                hull,
                geometry: Geometry::Glyph(source, found.linear, origin, &found.edges),
                colour: Colour::Paint(node.paint),
                stencil: false,
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

#[cfg(test)]
mod tests {
    use super::{Source, sources};
    use pagura_core::{ErrorKind, Meter};

    #[test]
    fn source_lookup_reserves_before_allocation() {
        let unit = size_of::<Option<Source>>();
        let mut slots = Vec::new();
        let mut bytes = Meter::new(2 * unit - 1, "test bytes");
        let error = sources(&mut slots, 2, &mut bytes).unwrap_err();
        assert_eq!(
            (error.kind, error.message),
            (ErrorKind::Limit, "test bytes")
        );
        assert_eq!((slots.len(), slots.capacity(), bytes.used()), (0, 0, 0));
        let error = sources(&mut slots, usize::MAX, &mut bytes).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit);
        assert_eq!((slots.capacity(), bytes.used()), (0, 0));

        let mut bytes = Meter::new(2 * unit, "test bytes");
        sources(&mut slots, 2, &mut bytes).unwrap();
        assert_eq!(slots.len(), 2);
        assert_eq!(bytes.used(), slots.capacity() * unit);
    }
}
