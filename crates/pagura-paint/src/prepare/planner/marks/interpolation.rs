//! Bilinear patterned stencils: sample opacity once at the device centre
//! (8.9.5.3, ADR 0004) after the pattern's jointly clipped geometry. The
//! source scales alpha and shape, independently of the graphics state's
//! AIS-controlled alpha/soft mask (11.6.4.4).

use super::{Colour, Covered, Geometry, Spec};
use crate::format::Format;
use crate::plan::{Compositing, GroupPlan, GroupUse, Item, ItemKind, Mark, Start, area};
use crate::prepare::Budget;
use crate::prepare::planner::{Planner, Scope};
use crate::source::{ImageBudgets, PaintContext, Source};
use pagura_core::{Error, Result};
use pagura_image::ImageKind;
use pagura_interpret::{BlendMode, Composite, ImageNode};
use pagura_raster::{DeviceRect, FillRule, PathBuilder, Point};

impl Planner<'_> {
    /// Prepares the ordinary bilinear image sampler without a colour. Its
    /// opacity is applied once to the whole tiling/shading result. The
    /// image's unit square remains geometry, so clipping and fractional
    /// pattern ink are still integrated jointly before quantization.
    pub(super) fn interpolated_stencil(
        &mut self,
        scope: &Scope,
        node: &ImageNode,
        state: &Composite,
        bounds: DeviceRect,
        budget: &mut Budget,
    ) -> Result<()> {
        if !matches!(node.image.source.kind(), ImageKind::Stencil { .. }) {
            return Err(Error::invalid_argument("pattern stencil is not a mask"));
        }
        let target = self.target(scope);
        let context = PaintContext {
            transform: scope.transform,
            target: &target,
            composite: state,
            store: self.store,
            limits: self.limits,
            region: bounds,
        };
        let source = Source::image(
            &node.image,
            node.transform.then(scope.transform),
            None,
            (&context, area(bounds)),
            ImageBudgets {
                bytes: &mut budget.bytes,
                work: &mut budget.work,
                functions: &mut budget.functions,
            },
        )?;
        let opacity = self.add_paint(source, bounds, None, budget)?;
        budget.work.charge_product(area(bounds), 2)?;
        if self.limits.path_segments < 5 {
            return Err(Error::limit("stencil path segment budget exceeded"));
        }
        let mut path = PathBuilder::new();
        path.reserve(5, &mut budget.bytes)?;
        path.polygon(
            [
                Point::new(0.0, 0.0),
                Point::new(1.0, 0.0),
                Point::new(1.0, 1.0),
                Point::new(0.0, 1.0),
            ]
            .map(|point| node.transform.apply(point)),
        );
        let path = path.finish();
        let released = path.retained_bytes();
        let spec = Spec {
            info: &node.mark,
            position: node.mark.position,
            hull: super::bounds::path(&path, scope.transform)
                .map(|h| super::bounds::expand(h, 1.0)),
            geometry: Geometry::Fill(&path, FillRule::NonZero),
            colour: Colour::Paint(
                node.stencil
                    .ok_or(Error::invalid_argument("stencil mask without a paint"))?,
            ),
            stencil: true,
            opacity: Some(opacity),
        };
        let result = self.prepare_mark(scope, &spec, budget);
        drop(path);
        budget.bytes.refund(released);
        result
    }

    /// A shading stencil's one mark becomes a group result so its sampled
    /// shape is applied at the same point as a tiling stencil's. The inner
    /// source keeps its colour conversion and overprint channel selection;
    /// original graphics-state compositing applies only on the result.
    /// Normal overprint inside the neutral group forms the implicit source
    /// `(1-Ab)Cs + Ab*Cb` (11.7.4.3). Its result must not substitute those
    /// components again when applying the original non-Normal blend.
    pub(super) fn sampled_stencil_mark(
        &mut self,
        scope: &Scope,
        spec: &Spec<'_>,
        covered: Covered,
        mut mark: Mark,
        opacity: u32,
        budget: &mut Budget,
    ) -> Result<()> {
        let compositing = mark.compositing;
        mark.compositing = Compositing {
            alpha: 1.0,
            blend: BlendMode::Normal,
            alpha_is_shape: false,
            mask: None,
            overprint: compositing.overprint,
        };
        mark.words = None;
        mark.records = false;
        let group = self.new_group(
            GroupPlan {
                bounds: covered.bounds,
                items: Vec::new(),
                order: Vec::new(),
                layout: scope.layout,
                format: Format::F64,
                knockout: false,
                start: Start::Backdrop,
            },
            budget,
        )?;
        budget.work.charge_product(area(covered.bounds), 3)?;
        let item = Item {
            bounds: covered.bounds,
            bands: self.bands(covered.bounds),
            serial: self.serial,
            position: spec.position,
            work: covered.work,
            kind: ItemKind::Mark(mark),
        };
        self.push(group, item, budget)?;
        let result = Item {
            bounds: covered.bounds,
            bands: self.bands(covered.bounds),
            serial: self.serial,
            position: spec.position,
            work: 0,
            kind: ItemKind::Group(GroupUse {
                group,
                cover: None,
                stencil: Some(opacity),
                compositing,
                conversion: None,
            }),
        };
        self.push(scope.group, result, budget)
    }
}
