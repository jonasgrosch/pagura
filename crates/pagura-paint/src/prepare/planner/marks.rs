//! Marks (8.5.3, 8.7, 8.9, 9.3.8): each is culled by conservative device
//! bounds first; a mark that meets the region then has its soft mask, clip,
//! shape and paint prepared and its work and bytes reserved, in that order,
//! so its errors come in source order. A glyph placement is a mark of its
//! own.

use super::clips::Clipping;
use super::{Planner, Scope};
use crate::paint::PaintSpans;
use crate::plan::{Compositing, Cover, GroupUse, Item, ItemKind, Mark, area, intersect};
use crate::prepare::{Backend, Budget, PaintRequest, Prepared, bounds, index};
use crate::source::{PaintContext, Source};
use pagura_color::ColorSpace;
use pagura_core::{Error, Position, Result};
use pagura_interpret::{
    BlendMode, Composite, FillNode, GlyphSource, GlyphsNode, ImageMask, ImageNode, MarkInfo, Paint,
    PaintId, Pen, ShadingNode, StrokeNode, TilingPaint,
};
use pagura_raster::{FillRule, LineJoin, Path, Point, ShapeOptions, Transform};
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
    Glyph(&'a GlyphSource, [f64; 4], Point),
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
    pub(super) fn fill(&mut self, scope: &Scope, node: &FillNode, budget: &mut Budget) -> Result<()> {
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
        let stroke = &pen.stroke;
        let join = if stroke.join == LineJoin::Miter {
            stroke.miter_limit.max(1.0)
        } else {
            1.0
        };
        let reach = stroke.width.max(0.0) / 2.0
            * join.max(std::f64::consts::SQRT_2)
            * bounds::scale(pen.transform.then(scope.transform));
        let hull = bounds::path(&node.path, scope.transform).map(|h| bounds::expand(h, reach + 2.0));
        let spec = Spec {
            info: &node.mark,
            position: node.mark.position,
            hull,
            geometry: Geometry::Stroke(&node.path, pen),
            colour: Colour::Paint(node.paint),
        };
        self.mark(scope, &spec, budget)
    }

    pub(super) fn glyphs(
        &mut self,
        scope: &Scope,
        node: &GlyphsNode,
        budget: &mut Budget,
    ) -> Result<()> {
        let t = scope.transform;
        let device = Transform::new(t.a, t.b, t.c, t.d, 0.0, 0.0);
        let mut hulls: Vec<Option<Option<bounds::Hull>>> = vec![None; node.sources.len()];
        for (i, placement) in node.placements.iter().enumerate() {
            let source_index = index(placement.source);
            let Some(source) = node.sources.get(source_index) else {
                self.serial += 1;
                return Err(self.fail(
                    Error::invalid_argument("glyph source out of range"),
                    node.position(i),
                ));
            };
            let l = source.linear;
            let linear = Transform::new(l.a, l.b, l.c, l.d, 0.0, 0.0).then(device);
            let relative = if let Some(hull) = hulls.get(source_index).copied().flatten() {
                hull
            } else {
                let hull = bounds::path(&source.outline, linear);
                if let Some(slot) = hulls.get_mut(source_index) {
                    *slot = Some(hull);
                }
                hull
            };
            let origin = t.apply(placement.origin);
            let hull = relative.map(|[x0, y0, x1, y1]| {
                bounds::expand([x0 + origin.x, y0 + origin.y, x1 + origin.x, y1 + origin.y], 1.0)
            });
            let spec = Spec {
                info: &node.mark,
                position: node.position(i),
                hull,
                geometry: Geometry::Glyph(source, [linear.a, linear.b, linear.c, linear.d], origin),
                colour: Colour::Paint(node.paint),
            };
            self.mark(scope, &spec, budget)?;
        }
        Ok(())
    }

    pub(super) fn image(&mut self, scope: &Scope, node: &ImageNode, budget: &mut Budget) -> Result<()> {
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
            let path = Path::rect(rect);
            let hull = bounds::path(&path, to_device).map(|h| bounds::expand(h, 1.0));
            (hull, Geometry::Area(Some((path, to_device))))
        } else {
            let r = self.area.region;
            let whole = [f64::from(r.x0), f64::from(r.y0), f64::from(r.x1), f64::from(r.y1)];
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
        let clip_bounds = self.clip_bounds(scope, spec.info.clip)?;
        let culling = intersect(
            intersect(bounds::device(spec.hull, self.area.region), clip_bounds),
            scope.bounds,
        );
        if culling.is_empty() {
            return Ok(());
        }
        let paint = match spec.colour {
            Colour::Paint(id) => Some(Arc::clone(
                scene
                    .paints
                    .get(id.index())
                    .ok_or(Error::invalid_argument("scene paint out of range"))?,
            )),
            Colour::Shading(_) | Colour::Image(_) => None,
        };
        let tiling = match paint.as_deref() {
            Some(Paint::Tiling(tiling)) if scope.paint.is_none() => Some(tiling.clone()),
            _ => None,
        };
        if self.storage.transfers && (tiling.is_some() || (self.simulate && state.overprint.enabled))
        {
            return Err(Error::unsupported(
                crate::features::TRANSPARENCY_TRANSFER,
                "transfer selection with patterns or simulated overprint is not supported",
            ));
        }
        let mask = state
            .soft_mask
            .map(|id| self.mask_use(scope, id, budget))
            .transpose()?;
        let Some(covered) = self.coverage(scope, spec, culling, budget)? else {
            return Ok(());
        };
        self.use_mask(mask, covered.bounds, budget)?;
        let compositing = Compositing {
            alpha: state.alpha,
            blend: state.blend,
            alpha_is_shape: state.alpha_is_shape,
            mask,
            overprint: if self.simulate && state.overprint.enabled {
                scope.layout.mask()
            } else {
                0
            },
            zeros: zeros(paint.as_deref(), spec.colour, &state, scope, self.simulate, self.scene),
        };
        match tiling {
            Some(tiling) => self.pattern_item(scope, spec, (&tiling, covered, compositing), budget),
            None => self.mark_item(scope, spec, (&state, covered, compositing), budget),
        }
    }

    /// The clip and shape of a mark that meets the region, and the pixels
    /// they cover; `None` when they cover none.
    fn coverage(
        &mut self,
        scope: &Scope,
        spec: &Spec<'_>,
        culling: pagura_raster::DeviceRect,
        budget: &mut Budget,
    ) -> Result<Option<Covered>> {
        let region = self.area.region;
        let (clip, clip_rect) = match self.clip(scope, spec.info.clip, budget)? {
            Clipping::Empty => return Ok(None),
            Clipping::All => (None, region),
            Clipping::Shape(prepared) => (Some(prepared.key), prepared.bounds),
        };
        let shape = match self.shape(scope, &spec.geometry)? {
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
        }))
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
        let paint = self.prepare_paint(scope, spec.colour, (state, state_id), area(covered.bounds), budget)?;
        let (records, select) = self.selection(scope, spec.colour, (state, state_id), budget)?;
        budget.work.charge(covered.work)?;
        budget.work.charge(area(covered.bounds))?;
        let item = Item {
            bounds: covered.bounds,
            bands: self.bands(covered.bounds),
            serial,
            position: spec.position,
            work: covered.work,
            kind: ItemKind::Mark(Mark {
                cover: covered.cover,
                paint,
                compositing,
                select,
                records,
            }),
        };
        self.push(scope.group, item, budget)
    }

    fn shape(&mut self, scope: &Scope, geometry: &Geometry<'_>) -> Result<Shape> {
        let options = ShapeOptions {
            area: self.area,
            precision: self.precision,
            limits: self.limits,
        };
        let prepared = match geometry {
            Geometry::Fill(path, rule) => self.backend.fill(path, scope.transform, *rule, &options)?,
            Geometry::Stroke(path, pen) => {
                let inverse = pen.transform.inverse().ok_or(Error::unsupported(
                    crate::features::SINGULAR_STROKE,
                    "singular stroke transform",
                ))?;
                self.backend
                    .stroke(path, (pen, inverse), scope.transform, &options)?
            }
            Geometry::Glyph(source, linear, origin) => {
                self.backend.glyph(source, *linear, *origin, &options)?
            }
            Geometry::Unit(transform) => {
                self.backend
                    .fill(&bounds::unit_square(), *transform, FillRule::NonZero, &options)?
            }
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
        pixels: usize,
        budget: &mut Budget,
    ) -> Result<u32> {
        let target = self.target(scope.layout);
        let context = PaintContext {
            transform: scope.transform,
            target: &target,
            composite: state,
            store: self.store,
            limits: self.limits,
            reduced_image_decode: self.request.fast.reduced_image_decode,
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
                let key = (id.0, state_id, scope.layout);
                if matches!(*paint, Paint::Solid(_))
                    && let Some(&cached) = self.solids().get(&key)
                {
                    return Ok(cached);
                }
                let prepared = self.backend.paint(&paint, &request, &mut budget.functions)?;
                let index = self.add_paint(prepared, scope, budget)?;
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
                    &mut budget.work,
                )?
            }
        };
        self.add_paint(prepared, scope, budget)
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
        if matches!(*paint, Paint::Tiling(_)) {
            return Err(Error::unsupported(
                crate::features::PATTERN_IMAGE_MASK,
                "an image mask painted with a tiling pattern is not supported",
            ));
        }
        budget.bytes.charge(PAINT_BYTES)?;
        self.backend.paint(&paint, request, &mut budget.functions)
    }

    fn add_paint(&mut self, paint: Box<dyn PaintSpans>, scope: &Scope, budget: &mut Budget) -> Result<u32> {
        budget
            .bytes
            .charge(PAINT_BYTES.saturating_add(scope.layout.channels() * size_of::<f64>()))?;
        let index = u32::try_from(self.parts.paints.len())
            .map_err(|_| Error::limit("paint count exceeds u32"))?;
        self.parts.paints.push(paint);
        Ok(index)
    }

    /// A solid paint for an uncoloured pattern's cell content.
    pub(crate) fn solid_paint(
        &mut self,
        paint: &Paint,
        (state, layout): (&Arc<Composite>, crate::plan::Layout),
        (transform, pixels): (Transform, usize),
        budget: &mut Budget,
    ) -> Result<u32> {
        let target = self.target(layout);
        let context = PaintContext {
            transform,
            target: &target,
            composite: state,
            store: self.store,
            limits: self.limits,
            reduced_image_decode: self.request.fast.reduced_image_decode,
        };
        let request = PaintRequest { context, pixels };
        let prepared = self.backend.paint(paint, &request, &mut budget.functions)?;
        let scope_layout = Scope {
            group: 0,
            transform,
            crop: false,
            bounds: self.area.region,
            layout,
            plain: false,
            records: false,
            paint: None,
            depth: 0,
        };
        self.add_paint(prepared, &scope_layout, budget)
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

/// The channels a direct DeviceCMYK colour leaves to its backdrop under
/// `OPM 1` (8.6.7): its zero components, when overprinting is simulated.
fn zeros(
    paint: Option<&Paint>,
    colour: Colour<'_>,
    state: &Composite,
    scope: &Scope,
    simulate: bool,
    scene: &pagura_interpret::Scene,
) -> u64 {
    if !(simulate
        && state.overprint.enabled
        && state.overprint.nonzero
        && scope.layout.process == pagura_color::Process::Cmyk)
    {
        return 0;
    }
    let paint = match (paint, colour) {
        (Some(paint), _) => Some(paint),
        (None, Colour::Image(node)) if node.image.space.is_none() => node
            .stencil
            .and_then(|id| scene.paints.get(id.index()))
            .map(|paint| &**paint),
        _ => None,
    };
    let Some(Paint::Solid(color)) = paint else {
        return 0;
    };
    if !matches!(*color.space, ColorSpace::DeviceCmyk) {
        return 0;
    }
    color
        .components
        .iter()
        .take(4)
        .enumerate()
        .filter(|(_, v)| v.clamp(0.0, 1.0) == 0.0)
        .fold(0, |bits, (i, _)| bits | (1 << i))
}
