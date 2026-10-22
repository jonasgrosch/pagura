//! Marks (8.5.3, 8.7, 8.9, 9.3.8): each is culled by conservative device
//! bounds first; a mark that meets the region then has its soft mask, clip,
//! shape and paint prepared and its work and bytes reserved, in that order,
//! so its errors come in source order. A glyph placement is a mark of its
//! own, gathered into its run's item where it can be (`runs`).

use super::clips::{Clipping, Extra};
use super::runs::{Memo, Placed};
use super::{Planner, Scope};
use crate::paint::PaintSpans;
use crate::plan::{Boundary, Compositing, Cover, GroupUse, Item, ItemKind, Mark, area, intersect};
use crate::prepare::overprint::{Keeping, Place, direct_cmyk};
use crate::prepare::{Backend, Budget, PaintRequest, Prepared, bounds, index};
use crate::source::{BBoxUse, PaintContext, Source};
use pagura_core::{Error, Limits, Position, Result};
use pagura_interpret::{
    BlendMode, Composite, FillNode, GlyphSource, GlyphsNode, ImageMask, ImageNode, MarkInfo, Paint,
    PaintId, Pen, ShadingNode, ShadingPaint, StrokeNode, TilingPaint,
};
use pagura_raster::{FillRegion, FillRule, Path, Point, ShapeOptions, Transform};
use std::sync::Arc;

/// Bytes charged per prepared paint, besides what its source retains.
const PAINT_BYTES: usize = size_of::<Source>() + size_of::<Box<dyn PaintSpans>>();

/// A mark's geometry.
enum Geometry<'a> {
    /// A fill.
    Fill(&'a Path, FillRule),
    /// A stroke.
    Stroke(&'a Path, &'a Pen),
    /// A glyph placement: its device linear map and origin.
    Glyph(&'a GlyphSource, [f64; 4], Point, usize),
    /// An image's unit square under this transform.
    Unit(Transform),
    /// A shading's `BBox` under its transform, or its clip region.
    Area(Option<(Path, Transform)>),
}

/// Where a mark's colour comes from.
#[derive(Clone, Copy)]
enum Colour<'a> {
    /// A paint.
    Paint(PaintId),
    /// A shading painted with `sh`.
    Shading(&'a ShadingNode),
    /// An image or stencil mask.
    Image(&'a ImageNode),
}

/// One mark to prepare.
struct Spec<'a> {
    info: &'a MarkInfo,
    position: Position,
    hull: Option<bounds::Hull>,
    geometry: Geometry<'a>,
    colour: Colour<'a>,
}

impl<B: Backend> Planner<'_, B> {
    pub(super) fn fill(
        &mut self,
        scope: &Scope,
        node: &FillNode,
        budget: &mut Budget,
    ) -> Result<()> {
        let hull = bounds::path(&node.path, scope.transform).map(|h| bounds::expand(h, 1.0));
        let spec = Spec {
            info: &node.mark,
            position: node.mark.position,
            hull,
            geometry: Geometry::Fill(&node.path, node.rule),
            colour: Colour::Paint(node.paint),
        };
        self.mark(scope, &spec, budget)
    }

    pub(super) fn stroke(
        &mut self,
        scope: &Scope,
        node: &StrokeNode,
        budget: &mut Budget,
    ) -> Result<()> {
        let pen = self
            .scene
            .pens
            .get(node.pen.index())
            .ok_or(Error::invalid_argument("scene pen out of range"))?;
        let reach = bounds::stroke_reach(pen, scope.transform);
        let hull =
            bounds::path(&node.path, scope.transform).map(|h| bounds::expand(h, reach + 2.0));
        let spec = Spec {
            info: &node.mark,
            position: node.mark.position,
            hull,
            geometry: Geometry::Stroke(&node.path, pen),
            colour: Colour::Paint(node.paint),
        };
        self.mark(scope, &spec, budget)
    }

    /// A glyph run: one item for its placements where they share a solid
    /// paint and need no transfer selection (`runs`), one per placement
    /// otherwise. Either way each placement is prepared, culled and charged
    /// as a mark of its own, so errors come in the same order.
    pub(super) fn glyphs(
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
                let hull = self.backend.glyph_hull((source, &mut edges), linear);
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
            let geometry = Geometry::Glyph(source, linear, origin, source_index);
            if let Some(memo) = self.memo.filter(|_| self.gathering == Some(scope.group)) {
                self.serial += 1;
                let placed = self.placement(scope, (&memo, hull, &geometry, position), budget);
                placed.map_err(|error| self.fail(error, position))?;
                continue;
            }
            let spec = Spec {
                info: &node.mark,
                position,
                hull,
                geometry,
                colour: Colour::Paint(node.paint),
            };
            self.mark(scope, &spec, budget)?;
        }
        Ok(())
    }

    pub(super) fn image(
        &mut self,
        scope: &Scope,
        node: &ImageNode,
        budget: &mut Budget,
    ) -> Result<()> {
        let to_device = node.transform.then(scope.transform);
        let hull = bounds::path(&bounds::unit_square(), to_device).map(|h| bounds::expand(h, 1.0));
        let spec = Spec {
            info: &node.mark,
            position: node.mark.position,
            hull,
            geometry: Geometry::Unit(to_device),
            colour: Colour::Image(node),
        };
        self.mark(scope, &spec, budget)
    }

    pub(super) fn shading(
        &mut self,
        scope: &Scope,
        node: &ShadingNode,
        budget: &mut Budget,
    ) -> Result<()> {
        let to_device = node.transform.then(scope.transform);
        let (hull, geometry) = if let Some(rect) = node.shading.bbox {
            let path = bbox_path(rect, self.limits)?;
            let hull = bounds::path(&path, to_device).map(|h| bounds::expand(h, 1.0));
            (hull, Geometry::Area(Some((path, to_device))))
        } else {
            let r = self.area.region;
            let whole = [
                f64::from(r.x0),
                f64::from(r.y0),
                f64::from(r.x1),
                f64::from(r.y1),
            ];
            (Some(whole), Geometry::Area(None))
        };
        let spec = Spec {
            info: &node.mark,
            position: node.mark.position,
            hull,
            geometry,
            colour: Colour::Shading(node),
        };
        self.mark(scope, &spec, budget)
    }

    fn mark(&mut self, scope: &Scope, spec: &Spec<'_>, budget: &mut Budget) -> Result<()> {
        self.serial += 1;
        let prepared = self.prepare_mark(scope, spec, budget);
        prepared.map_err(|error| self.fail(error, spec.position))
    }

    fn prepare_mark(&mut self, scope: &Scope, spec: &Spec<'_>, budget: &mut Budget) -> Result<()> {
        let scene = self.scene;
        let state = Arc::clone(
            scene
                .states
                .get(spec.info.state.index())
                .ok_or(Error::invalid_argument("scene state out of range"))?,
        );
        let paint = match spec.colour {
            Colour::Paint(id) => {
                Some(Arc::clone(scene.paints.get(id.index()).ok_or(
                    Error::invalid_argument("scene paint out of range"),
                )?))
            }
            Colour::Shading(_) | Colour::Image(_) => None,
        };
        // With a transfer function anywhere outside soft masks, a pattern or
        // a simulated overprint outside them needs per-region selection,
        // which is not implemented.
        let patterned = scope.paint.is_none() && paint.as_deref().is_some_and(is_pattern);
        let policy = self.request.overprint == crate::request::OverprintMode::Simulate;
        if self.storage.transfer_gate
            && !scope.masked
            && (patterned || (policy && state.overprint.enabled))
        {
            return Err(Error::unsupported(
                pagura_interpret::features::TRANSPARENCY_TRANSFER,
                "per-colourant and pattern transfer-region selection is not implemented",
            ));
        }
        if !paints(paint.as_deref(), spec.colour, scope, scene) {
            return Ok(());
        }
        let tiling = match paint.as_deref() {
            Some(Paint::Tiling(tiling)) if scope.paint.is_none() => Some(tiling.clone()),
            _ => None,
        };
        let pattern_box = match (paint.as_deref(), spec.colour) {
            (Some(Paint::Shading(pattern)), Colour::Paint(id)) if scope.paint.is_none() => {
                pattern_bbox(pattern, scope.transform, self.limits)?.map(|(usage, extra)| {
                    (
                        usage,
                        Extra {
                            paint: id.0,
                            ..extra
                        },
                    )
                })
            }
            _ => None,
        };
        let extra = pattern_box
            .as_ref()
            .filter(|(usage, _)| *usage == BBoxUse::Clip)
            .map(|(_, extra)| extra);
        let clip_bounds = self.clip_bounds(scope, spec.info.clip, extra)?;
        let culling = intersect(
            intersect(bounds::device(spec.hull, self.area.region), clip_bounds),
            scope.bounds,
        );
        if culling.is_empty() {
            return Ok(());
        }
        let mask = state
            .soft_mask
            .map(|id| self.mask_use(scope, id, budget))
            .transpose()?;
        let Some(mut covered) = self.coverage(scope, spec, (culling, extra), budget)? else {
            return Ok(());
        };
        if let Some((BBoxUse::Boundary, boundary)) = &pattern_box {
            self.boundary(&mut covered, boundary, budget)?;
        }
        self.use_mask(mask, covered.bounds, budget)?;
        let compositing = Compositing {
            alpha: state.alpha,
            blend: state.blend,
            alpha_is_shape: state.alpha_is_shape,
            mask,
            overprint: if self.keeps(scope, spec.colour, &state) {
                scope.layout.mask()
            } else {
                0
            },
        };
        match tiling {
            Some(tiling) => self.pattern_item(scope, spec, (&tiling, covered, compositing), budget),
            None => self.mark_item(scope, spec, (&state, covered, compositing), budget),
        }
    }

    /// A glyph placement of the run being gathered, with what its run's
    /// first prepared placement found: culled, its shape prepared within
    /// the clip state, its mask use and work charged, then gathered.
    fn placement(
        &mut self,
        scope: &Scope,
        (memo, hull, geometry, position): (&Memo, Option<bounds::Hull>, &Geometry<'_>, Position),
        budget: &mut Budget,
    ) -> Result<()> {
        let region = self.area.region;
        let culling = intersect(
            intersect(bounds::device(hull, region), memo.clip_bounds),
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
        self.backend.shapes_clip(clip);
        let shape = self.shape(scope, geometry);
        self.backend.shapes_clip(None);
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
            position,
        };
        self.gather(scope.group, mark, placed, budget)
    }

    /// The clip and shape of a mark that meets the region, and the pixels
    /// they cover; `None` when they cover none.
    fn coverage(
        &mut self,
        scope: &Scope,
        spec: &Spec<'_>,
        (culling, extra): (pagura_raster::DeviceRect, Option<&Extra>),
        budget: &mut Budget,
    ) -> Result<Option<Covered>> {
        let region = self.area.region;
        let (clip, clip_rect) = match self.clip(scope, (spec.info.clip, extra), budget)? {
            Clipping::Empty => return Ok(None),
            Clipping::All => (None, region),
            Clipping::Shape(prepared) => (Some(prepared.key), prepared.bounds),
        };
        self.backend.shapes_clip(clip);
        let shape = self.shape(scope, &spec.geometry);
        self.backend.shapes_clip(None);
        let shape = match shape? {
            Shape::Nothing => return Ok(None),
            Shape::Everything => None,
            Shape::Prepared(prepared) => Some(prepared),
        };
        let bounds = intersect(
            intersect(shape.map_or(region, |s| s.bounds), clip_rect),
            intersect(scope.bounds, culling),
        );
        if bounds.is_empty() {
            return Ok(None);
        }
        budget.bytes.charge(shape.map_or(0, |s| s.bytes))?;
        Ok(Some(Covered {
            cover: Cover {
                shape: shape.map(|s| s.key),
                clip,
            },
            bounds,
            work: shape.map_or(0, |s| s.work),
            boundary: Boundary::Absent,
        }))
    }

    /// Whether the mark keeps the backdrop's channels where its colour does
    /// not paint them.
    fn keeps(&self, scope: &Scope, colour: Colour<'_>, state: &Composite) -> bool {
        let paint = |id: PaintId| match &scope.paint {
            Some((_, paint)) => Some(Arc::clone(paint)),
            None => self.scene.paints.get(id.index()).cloned(),
        };
        let direct = match colour {
            Colour::Paint(id) => paint(id),
            Colour::Image(node) if node.image.space.is_none() => node.stencil.and_then(paint),
            Colour::Image(_) | Colour::Shading(_) => None,
        };
        let keeping = Keeping {
            separated: self.simulate,
            policy: self.request.overprint == crate::request::OverprintMode::Simulate,
        };
        keeping.keeps(
            state,
            Place {
                masked: scope.masked,
                cmyk: scope.layout.process == pagura_color::Process::Cmyk,
                direct_cmyk: direct.as_deref().is_some_and(direct_cmyk),
            },
        )
    }

    /// The outline coverage a shading pattern with a `Background` and a
    /// degenerate `BBox` mixes its colours by (8.7.4.4).
    fn boundary(
        &mut self,
        covered: &mut Covered,
        boundary: &Extra,
        budget: &mut Budget,
    ) -> Result<()> {
        let options = ShapeOptions {
            area: self.area,
            limits: self.limits,
        };
        let prepared = self.backend.fill(
            &boundary.path,
            boundary.transform,
            FillRule::NonZero,
            &options,
        )?;
        covered.boundary = match prepared {
            Some(prepared) => {
                budget.bytes.charge(prepared.bytes)?;
                covered.work = covered.work.saturating_add(prepared.work);
                Boundary::Shape(prepared.key)
            }
            // Away from the outline the background alone shows.
            None => Boundary::Empty,
        };
        Ok(())
    }

    /// A mark painted with a tiling pattern: the pattern's instances as a
    /// group composited through the mark's coverage.
    fn pattern_item(
        &mut self,
        scope: &Scope,
        spec: &Spec<'_>,
        (tiling, covered, compositing): (&TilingPaint, Covered, Compositing),
        budget: &mut Budget,
    ) -> Result<()> {
        let serial = self.serial;
        let Some((group, content)) = self.pattern(scope, tiling, covered.bounds, budget)? else {
            return Ok(());
        };
        budget.work.charge(covered.work)?;
        budget.work.charge_product(area(content), 3)?;
        let item = Item {
            bounds: content,
            bands: self.bands(content),
            serial,
            position: spec.position,
            work: covered.work,
            kind: ItemKind::Group(GroupUse {
                group,
                cover: Some(covered.cover),
                compositing,
                conversion: None,
            }),
        };
        self.push(scope.group, item, budget)
    }

    /// A mark with its prepared paint.
    fn mark_item(
        &mut self,
        scope: &Scope,
        spec: &Spec<'_>,
        (state, covered, compositing): (&Arc<Composite>, Covered, Compositing),
        budget: &mut Budget,
    ) -> Result<()> {
        let serial = self.serial;
        let state_id = spec.info.state.0;
        let paint = self.prepare_paint(
            scope,
            spec.colour,
            (state, state_id),
            covered.bounds,
            budget,
        )?;
        let (records, select) = self.selection(scope, spec.colour, (state, state_id), budget)?;
        let words = self
            .parts
            .paints
            .get(index(paint))
            .and_then(|paint| paint.constant())
            .filter(|color| color.len() >= 3 && compositing.is_opaque())
            .map(|c| {
                let word = crate::kernel::word;
                [word(c[0]), word(c[1]), word(c[2]), u16::MAX]
            });
        budget.work.charge(covered.work)?;
        budget.work.charge(area(covered.bounds))?;
        let mark = Mark {
            cover: covered.cover,
            paint,
            compositing,
            select,
            records,
            boundary: covered.boundary,
            words,
        };
        if self.gathering == Some(scope.group)
            && !records
            && covered.boundary == Boundary::Absent
            && let Some(shape) = covered.cover.shape
        {
            let placed = Placed {
                shape,
                bounds: covered.bounds,
                work: covered.work,
                serial,
                position: spec.position,
            };
            if self.memo.is_none() {
                let clip_bounds = self.clip_bounds(scope, spec.info.clip, None)?;
                let clipping = self.clip(scope, (spec.info.clip, None), budget)?;
                self.memo = Some(Memo {
                    clip_bounds,
                    clipping,
                    mask: compositing.mask,
                    mark,
                });
            }
            return self.gather(scope.group, mark, placed, budget);
        }
        let item = Item {
            bounds: covered.bounds,
            bands: self.bands(covered.bounds),
            serial,
            position: spec.position,
            work: covered.work,
            kind: ItemKind::Mark(mark),
        };
        self.push(scope.group, item, budget)
    }

    fn shape(&mut self, scope: &Scope, geometry: &Geometry<'_>) -> Result<Shape> {
        let options = ShapeOptions {
            area: self.area,
            limits: self.limits,
        };
        let prepared = match geometry {
            Geometry::Fill(path, rule) => {
                self.backend.fill(path, scope.transform, *rule, &options)?
            }
            Geometry::Stroke(path, pen) => {
                let inverse = pen.transform.inverse().ok_or(Error::unsupported(
                    pagura_interpret::features::SINGULAR_STROKE,
                    "singular stroke transform",
                ))?;
                self.backend
                    .stroke(path, (pen, inverse), scope.transform, &options)?
            }
            Geometry::Glyph(source, linear, origin, index) => {
                let mut edges = self.glyph_edges.get(*index).copied().flatten();
                let prepared =
                    self.backend
                        .glyph((source, &mut edges), *linear, *origin, &options)?;
                if let Some(slot) = self.glyph_edges.get_mut(*index) {
                    *slot = edges;
                }
                prepared
            }
            Geometry::Unit(transform) => self.backend.fill(
                &bounds::unit_square(),
                *transform,
                FillRule::NonZero,
                &options,
            )?,
            Geometry::Area(Some((path, transform))) => {
                self.backend
                    .fill(path, *transform, FillRule::NonZero, &options)?
            }
            Geometry::Area(None) => return Ok(Shape::Everything),
        };
        Ok(prepared.map_or(Shape::Nothing, Shape::Prepared))
    }

    /// The paint of a mark, prepared once per solid paint, state and
    /// target, and per mark otherwise.
    fn prepare_paint(
        &mut self,
        scope: &Scope,
        colour: Colour<'_>,
        (state, state_id): (&Arc<Composite>, u32),
        bounds: pagura_raster::DeviceRect,
        budget: &mut Budget,
    ) -> Result<u32> {
        let pixels = area(bounds);
        let target = self.target(scope);
        let context = PaintContext {
            transform: scope.transform,
            target: &target,
            composite: state,
            store: self.store,
            limits: self.limits,
            reduced_image_decode: self.request.fast.reduced_image_decode,
            region: bounds,
        };
        let request = PaintRequest { context, pixels };
        let prepared = match colour {
            Colour::Paint(id) => {
                if let Some((paint, _)) = scope.paint.as_ref() {
                    return Ok(*paint);
                }
                let paint = Arc::clone(
                    self.scene
                        .paints
                        .get(id.index())
                        .ok_or(Error::invalid_argument("scene paint out of range"))?,
                );
                let key = (id.0, state_id, scope.layout, target.colorants);
                if matches!(*paint, Paint::Solid(_))
                    && let Some(&cached) = self.solids().get(&key)
                {
                    return Ok(cached);
                }
                let prepared = self
                    .backend
                    .paint(&paint, &request, &mut budget.functions)?;
                let index = self.add_paint(prepared, scope, pixels, budget)?;
                if matches!(*paint, Paint::Solid(_)) {
                    self.solids().insert(key, index);
                }
                return Ok(index);
            }
            Colour::Shading(node) => self.backend.shading(
                &node.shading,
                node.transform.then(scope.transform),
                &request,
                &mut budget.functions,
            )?,
            Colour::Image(node) => {
                let stencil = match (&node.image.space, node.stencil) {
                    (Some(_), _) => None,
                    (None, stencil) => Some(self.stencil(scope, stencil, &request, budget)?),
                };
                self.backend.image(
                    &node.image,
                    node.transform.then(scope.transform),
                    stencil,
                    &request,
                    (&mut budget.work, &mut budget.functions),
                )?
            }
        };
        self.add_paint(prepared, scope, pixels, budget)
    }

    /// The colour of a stencil mask (8.9.6.2): its fill paint, or the colour
    /// of an uncoloured pattern's cell.
    fn stencil(
        &mut self,
        scope: &Scope,
        stencil: Option<PaintId>,
        request: &PaintRequest<'_>,
        budget: &mut Budget,
    ) -> Result<Box<dyn PaintSpans>> {
        let paint = match (&scope.paint, stencil) {
            (Some((_, paint)), _) => Arc::clone(paint),
            (None, Some(id)) => Arc::clone(
                self.scene
                    .paints
                    .get(id.index())
                    .ok_or(Error::invalid_argument("scene paint out of range"))?,
            ),
            (None, None) => return Err(Error::invalid_argument("stencil mask without a paint")),
        };
        if is_pattern(&paint) {
            return Err(Error::unsupported(
                pagura_interpret::features::PATTERN_IMAGE_MASK,
                "pattern-coloured image stencils are not implemented",
            ));
        }
        budget.bytes.charge(PAINT_BYTES)?;
        self.backend.paint(&paint, request, &mut budget.functions)
    }

    /// Keeps a prepared paint, reserving its bytes and its geometry work
    /// over `pixels` (mesh shadings).
    fn add_paint(
        &mut self,
        paint: Box<dyn PaintSpans>,
        scope: &Scope,
        pixels: usize,
        budget: &mut Budget,
    ) -> Result<u32> {
        budget.bytes.charge(
            PAINT_BYTES
                .saturating_add(scope.layout.channels() * size_of::<f64>())
                .saturating_add(paint.bytes()),
        )?;
        budget.work.charge_product(pixels, paint.work_per_pixel())?;
        let repairs = paint.repairs();
        budget
            .bytes
            .charge_product(repairs.len(), size_of::<pagura_core::Diagnostic>())?;
        self.repairs.extend_from_slice(repairs);
        let index = u32::try_from(self.parts.paints.len())
            .map_err(|_| Error::limit("paint count exceeds u32"))?;
        self.parts.paints.push(paint);
        Ok(index)
    }

    /// A solid paint for an uncoloured pattern's cell content.
    pub(crate) fn solid_paint(
        &mut self,
        paint: &Paint,
        (state, scope): (&Arc<Composite>, &Scope),
        bounds: pagura_raster::DeviceRect,
        budget: &mut Budget,
    ) -> Result<u32> {
        let pixels = area(bounds);
        let target = self.target(scope);
        let context = PaintContext {
            transform: scope.transform,
            target: &target,
            composite: state,
            store: self.store,
            limits: self.limits,
            reduced_image_decode: self.request.fast.reduced_image_decode,
            region: bounds,
        };
        let request = PaintRequest { context, pixels };
        let prepared = self.backend.paint(paint, &request, &mut budget.functions)?;
        self.add_paint(prepared, scope, pixels, budget)
    }

    /// Whether the mark records a transfer selection and which (11.7.5.2):
    /// the default unless it and every enclosing group composite opaquely
    /// and its state has transfer functions.
    fn selection(
        &mut self,
        scope: &Scope,
        colour: Colour<'_>,
        (state, state_id): (&Arc<Composite>, u32),
        budget: &mut Budget,
    ) -> Result<(bool, u16)> {
        if !scope.records {
            return Ok((false, 0));
        }
        let eligible = !matches!(colour, Colour::Image(node)
            if matches!(node.image.mask, ImageMask::Soft { .. } | ImageMask::Embedded));
        let opaque = scope.plain
            && state.alpha == 1.0
            && state.blend == BlendMode::Normal
            && state.soft_mask.is_none()
            && eligible;
        let select = match (&state.transfer, opaque) {
            (Some(transfer), true) => self.transfer(state_id, transfer, budget)?,
            _ => 0,
        };
        Ok((true, select))
    }
}

/// What a mark covers.
#[derive(Debug, Clone, Copy)]
struct Covered {
    cover: Cover,
    bounds: pagura_raster::DeviceRect,
    work: usize,
    /// A shading pattern's outline shape, for its `Background`.
    boundary: Boundary,
}

/// How a shading pattern's `BBox` applies, decided as its source decides
/// it: no `BBox` limits nothing; without a `Background` the mark is clipped
/// to the box; with one, a degenerate box mixes colours by its outline's
/// coverage and any other box is tested per sample by the source.
fn pattern_bbox(
    pattern: &ShadingPaint,
    transform: Transform,
    limits: &Limits,
) -> Result<Option<(BBoxUse, Extra)>> {
    let Some(bbox) = pattern.shading.bbox else {
        return Ok(None);
    };
    let path = bbox_path(bbox, limits)?;
    let degenerate = path
        .fill_region(FillRule::NonZero)
        .is_some_and(|region| region != FillRegion::Ordinary);
    let usage = if pattern.shading.background.is_none() {
        BBoxUse::Clip
    } else if degenerate {
        BBoxUse::Boundary
    } else {
        return Ok(None);
    };
    Ok(Some((
        usage,
        Extra {
            paint: 0,
            path: Arc::new(path),
            transform: pattern.matrix.then(transform),
        },
    )))
}

/// A shading's `BBox` as a path, classified so that a box without area
/// clips and fills as its device hairline outline, as a clipping path does.
fn bbox_path(bbox: pagura_core::Rect, limits: &Limits) -> Result<Path> {
    let mut path = Path::rect(bbox);
    path.classify_fill(FillRule::NonZero, limits, &mut 0)?;
    Ok(path)
}

/// Whether a mark's colour space can change any colorant (Separation
/// `None` cannot, 8.6.6.4).
fn paints(
    paint: Option<&Paint>,
    colour: Colour<'_>,
    scope: &Scope,
    scene: &pagura_interpret::Scene,
) -> bool {
    // A Pattern colour space before any `scn` selects a pattern paints
    // nothing; a stencil mask in it is refused when prepared.
    let of = |paint: &Paint| match paint {
        Paint::Solid(color) => color.space.paints() && !is_pattern(paint),
        Paint::Shading(pattern) => pattern.shading.space.paints(),
        Paint::Tiling(_) => true,
    };
    let stencil = |node: &ImageNode| {
        node.stencil
            .and_then(|id| scene.paints.get(id.index()))
            .is_none_or(|paint| is_pattern(paint) || of(paint))
    };
    match (colour, &scope.paint) {
        (Colour::Image(node), _) if node.image.space.is_some() => {
            node.image.space.as_ref().is_none_or(|space| space.paints())
        }
        (Colour::Paint(_) | Colour::Image(_), Some((_, paint))) => of(paint),
        (Colour::Paint(_), None) => paint.is_none_or(of),
        (Colour::Image(node), None) => stencil(node),
        (Colour::Shading(node), _) => node.shading.space.paints(),
    }
}

/// Whether `paint` is a pattern: tiling, shading, or a Pattern colour
/// space with no pattern selected.
fn is_pattern(paint: &Paint) -> bool {
    match paint {
        Paint::Solid(color) => matches!(*color.space, pagura_color::ColorSpace::Pattern(_)),
        Paint::Shading(_) | Paint::Tiling(_) => true,
    }
}

/// A mark's shape.
enum Shape {
    /// It covers no pixel of the area.
    Nothing,
    /// It covers its clip region.
    Everything,
    /// A prepared shape.
    Prepared(Prepared),
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
