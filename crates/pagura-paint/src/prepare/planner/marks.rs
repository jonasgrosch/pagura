//! Marks (8.5.3, 8.7, 8.9, 9.3.8): each is culled by conservative device
//! bounds first; a mark that meets the region then has its soft mask, clip,
//! shape and paint prepared and its work and bytes reserved, in that order,
//! so its errors come in source order. A glyph placement is a mark of its
//! own, gathered into its run's item where it can be (`runs`).

mod glyphs;
mod paints;

use super::clips::{Clipping, Extra};
use super::runs::{Memo, Placed};
use super::{Planner, Scope};
use crate::plan::{Boundary, Compositing, Cover, GroupUse, Item, ItemKind, Mark, area, intersect};
use crate::prepare::overprint::{Keeping, Place, direct_cmyk};
use crate::prepare::{Budget, Prepared, bounds, index};
use crate::source::{BBoxUse, Source};
use pagura_core::{Error, Limits, Meter, Position, Rect, Result};
use pagura_interpret::{
    BlendMode, Composite, FillNode, GlyphSource, ImageMask, ImageNode, MarkInfo, Paint, PaintId,
    Pen, ShadingNode, ShadingPaint, StrokeNode, TilingPaint,
};
use pagura_raster::{FillRule, Path, Point, ShapeOptions, Transform};
use std::cell::Cell;
use std::sync::Arc;

/// A mark's geometry.
enum Geometry<'a> {
    /// A fill.
    Fill(&'a Path, FillRule),
    /// A stroke.
    Stroke(&'a Path, &'a Pen),
    /// A glyph placement: its device linear map and origin.
    Glyph(&'a GlyphSource, [f64; 4], Point, &'a Cell<Option<u32>>),
    /// An image's unit square under this transform.
    Unit(Transform),
    /// A shading's `BBox` under its transform, or its clip region.
    Area(Option<(Rect, Transform)>),
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

impl Planner<'_> {
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

    pub(super) fn image(
        &mut self,
        scope: &Scope,
        node: &ImageNode,
        budget: &mut Budget,
    ) -> Result<()> {
        let to_device = node.transform.then(scope.transform);
        let hull = bounds::rect(unit_rect(), to_device).map(|h| bounds::expand(h, 1.0));
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
            let hull = bounds::rect(rect, to_device).map(|h| bounds::expand(h, 1.0));
            (hull, Geometry::Area(Some((rect, to_device))))
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
                pattern_bbox(pattern, scope.transform).map(|(usage, extra)| {
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
            Some(tiling) => {
                self.pattern_item((scope, spec), &tiling, (covered, compositing), budget)
            }
            None => self.mark_item((scope, spec), &state, (covered, compositing), budget),
        }
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
        self.shapes.set_clip(clip);
        let shape = self.shape(scope, &spec.geometry, budget);
        self.shapes.set_clip(None);
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
        let prepared = self.rect_shape(boundary.bbox, boundary.transform, true, budget)?;
        covered.boundary = match prepared {
            Some(prepared) => {
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
        (scope, spec): (&Scope, &Spec<'_>),
        tiling: &TilingPaint,
        (covered, compositing): (Covered, Compositing),
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
        (scope, spec): (&Scope, &Spec<'_>),
        state: &Arc<Composite>,
        (covered, compositing): (Covered, Compositing),
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
        // An image stores dynamically even where every pixel takes one
        // colour, as the storage survey counts it (ADR 0004 §6).
        let words = self
            .parts
            .paints
            .get(index(paint))
            .and_then(Source::constant)
            .filter(|(color, _)| {
                color.len() >= 3
                    && compositing.is_opaque()
                    && !matches!(spec.colour, Colour::Image(_))
            })
            .map(|(c, _)| {
                let word = crate::kernel::word;
                [word(c[0]), word(c[1]), word(c[2]), u16::MAX]
            });
        budget.work.charge(covered.work)?;
        budget.work.charge(area(covered.bounds))?;
        if records {
            // The same rasterization validates transfer selections before
            // execution. Reserve it in source order beside this mark's draw.
            budget.work.charge(covered.work)?;
        }
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

    fn shape(
        &mut self,
        scope: &Scope,
        geometry: &Geometry<'_>,
        budget: &mut Budget,
    ) -> Result<Shape> {
        let limits = Limits {
            render_bytes: budget.bytes.remaining(),
            ..*self.limits
        };
        let options = ShapeOptions {
            area: self.area,
            limits: &limits,
        };
        let prepared = match geometry {
            Geometry::Fill(path, rule) => {
                self.shapes
                    .fill(path, scope.transform, *rule, &options, &mut budget.bytes)?
            }
            Geometry::Stroke(path, pen) => {
                let inverse = pen.transform.inverse().ok_or(Error::unsupported(
                    pagura_interpret::features::SINGULAR_STROKE,
                    "singular stroke transform",
                ))?;
                self.shapes.stroke(
                    path,
                    (pen, inverse),
                    scope.transform,
                    &options,
                    &mut budget.bytes,
                )?
            }
            Geometry::Glyph(source, linear, origin, handle) => {
                let mut edges = handle.get();
                let prepared = self.shapes.glyph(
                    (source, &mut edges),
                    *linear,
                    *origin,
                    &options,
                    &mut budget.bytes,
                )?;
                handle.set(edges);
                prepared
            }
            Geometry::Unit(transform) => self.rect_shape(unit_rect(), *transform, false, budget)?,
            Geometry::Area(Some((rect, transform))) => {
                self.rect_shape(*rect, *transform, true, budget)?
            }
            Geometry::Area(None) => return Ok(Shape::Everything),
        };
        Ok(prepared.map_or(Shape::Nothing, Shape::Prepared))
    }

    /// Fixed geometry is built only after culling. Its path stays charged
    /// while the coverage source is prepared beside it, then is released.
    fn rect_shape(
        &mut self,
        rect: Rect,
        transform: Transform,
        classify: bool,
        budget: &mut Budget,
    ) -> Result<Option<Prepared>> {
        let path = if classify {
            bbox_path(rect, self.limits, &mut budget.bytes)?
        } else {
            Path::rect_with_bytes(rect, &mut budget.bytes)?
        };
        let limits = Limits {
            render_bytes: budget.bytes.remaining(),
            ..*self.limits
        };
        let options = ShapeOptions {
            area: self.area,
            limits: &limits,
        };
        let prepared = self.shapes.fill(
            &path,
            transform,
            FillRule::NonZero,
            &options,
            &mut budget.bytes,
        );
        let released = path.retained_bytes();
        drop(path);
        budget.bytes.refund(released);
        prepared
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
fn pattern_bbox(pattern: &ShadingPaint, transform: Transform) -> Option<(BBoxUse, Extra)> {
    let bbox = pattern.shading.bbox?;
    // Rect is normalized and finite. Its canonical four-corner contour
    // has interior exactly when both coordinate intervals are nonempty.
    let usage = if pattern.shading.background.is_none() {
        BBoxUse::Clip
    } else if bbox.is_empty() {
        BBoxUse::Boundary
    } else {
        return None;
    };
    Some((
        usage,
        Extra {
            paint: 0,
            bbox,
            transform: pattern.matrix.then(transform),
        },
    ))
}

/// A shading's `BBox` as a path, classified so that a box without area
/// clips and fills as its device hairline outline, as a clipping path does.
pub(super) fn bbox_path(bbox: Rect, limits: &Limits, bytes: &mut Meter) -> Result<Path> {
    let mut path = Path::rect_with_bytes(bbox, bytes)?;
    let classified = path.classify_fill_with_bytes(FillRule::NonZero, limits, &mut 0, bytes);
    if let Err(error) = classified {
        let released = path.retained_bytes();
        drop(path);
        bytes.refund(released);
        return Err(error);
    }
    Ok(path)
}

fn unit_rect() -> Rect {
    Rect::from_corners(0.0, 0.0, 1.0, 1.0).unwrap_or_else(|| unreachable!())
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

#[cfg(test)]
mod tests {
    use super::*;
    use pagura_raster::FillRegion;

    #[test]
    fn bbox_choice_matches_canonical_classification_for_slivers_and_degenerate_rects() {
        let limits = Limits::default();
        for (x, y, width, height) in [
            (0.0, 0.0, 1.0, 1.0),
            (0.0, 0.0, 1e-200, 1e-200), // area underflows, endpoints do not
            (0.0, 0.0, f64::from_bits(1), f64::from_bits(1)),
            (-2.0, 3.0, 0.0, 4.0),
            (-2.0, 3.0, 4.0, 0.0),
            (-2.0, 3.0, 0.0, 0.0),
        ] {
            let rect = Rect::from_corners(x, y, x + width, y + height).unwrap();
            let mut bytes = Meter::new(1 << 20, "bbox memory");
            let path = bbox_path(rect, &limits, &mut bytes).unwrap();
            let region = path.fill_region(FillRule::NonZero).unwrap();
            assert_eq!(rect.is_empty(), region != FillRegion::Ordinary, "{rect:?}");
            assert_eq!(bytes.used(), path.retained_bytes());
        }
        let mut tiny = Meter::new(5 * size_of::<pagura_raster::Segment>() - 1, "bbox memory");
        assert_eq!(
            bbox_path(unit_rect(), &limits, &mut tiny)
                .unwrap_err()
                .message,
            "bbox memory"
        );
        assert_eq!(tiny.used(), 0);
    }
}
