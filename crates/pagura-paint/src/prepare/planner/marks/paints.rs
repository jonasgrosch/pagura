//! Prepared paint of marks: solids once per paint, state and target,
//! shadings and images per mark, stencil masks with their solid colour,
//! each reserved with its bytes, geometry work and repairs.

use super::{Colour, PAINT_BYTES, is_pattern};
use crate::plan::area;
use crate::prepare::Budget;
use crate::prepare::planner::{Planner, Scope, SolidKey};
use crate::source::{ImageBudgets, PaintContext, Source};
use pagura_color::Painted;
use pagura_core::{Diagnostic, Error, Result};
use pagura_interpret::{Composite, Paint, PaintId};
use std::sync::Arc;

impl Planner<'_> {
    /// The paint of a mark, prepared once per solid paint, state and
    /// target, and per mark otherwise.
    pub(super) fn prepare_paint(
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
            region: bounds,
        };
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
                let key = SolidKey {
                    paint: id.0,
                    state: state_id,
                    layout: scope.layout,
                    colorants: target.colorants,
                };
                if matches!(*paint, Paint::Solid(_))
                    && let Some(&cached) = self.solids().get(&key)
                {
                    return Ok(cached);
                }
                let prepared = Source::prepare(
                    &paint,
                    &context,
                    pixels,
                    (&mut budget.functions, &budget.bytes),
                )?;
                let index = self.add_paint(prepared, (scope, pixels), budget)?;
                if matches!(*paint, Paint::Solid(_)) {
                    self.solids().insert(key, index);
                }
                return Ok(index);
            }
            Colour::Shading(node) => {
                let to_device = node.transform.then(scope.transform);
                Source::shading(
                    &node.shading,
                    to_device,
                    &context,
                    pixels,
                    (&mut budget.functions, &budget.bytes),
                )?
            }
            Colour::Image(node) => {
                let stencil = match (&node.image.space, node.stencil) {
                    (Some(_), _) => None,
                    (None, stencil) => Some(self.stencil(scope, stencil, &context, budget)?),
                };
                Source::image(
                    &node.image,
                    node.transform.then(scope.transform),
                    stencil
                        .as_ref()
                        .map(|(color, painted)| (&**color, *painted)),
                    (&context, pixels),
                    ImageBudgets {
                        bytes: &mut budget.bytes,
                        work: &mut budget.work,
                        functions: &mut budget.functions,
                    },
                )?
            }
        };
        self.add_paint(prepared, (scope, pixels), budget)
    }

    /// The colour of a stencil mask (8.9.6.2): its fill paint, or the colour
    /// of an uncoloured pattern's cell, which is a solid colour.
    fn stencil(
        &mut self,
        scope: &Scope,
        stencil: Option<PaintId>,
        context: &PaintContext<'_>,
        budget: &mut Budget,
    ) -> Result<(Box<[f64]>, Painted)> {
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
        let color = match &*paint {
            Paint::Solid(color) if !is_pattern(&paint) => color,
            _ => {
                return Err(Error::unsupported(
                    pagura_interpret::features::PATTERN_IMAGE_MASK,
                    "pattern-coloured image stencils are not implemented",
                ));
            }
        };
        budget.bytes.charge(PAINT_BYTES)?;
        Source::solid_color(color, context, &mut budget.functions)
    }

    /// Keeps a prepared paint, reserving its bytes and its geometry work
    /// over `pixels` (mesh shadings), and the repairs preparing it made.
    fn add_paint(
        &mut self,
        paint: Source,
        (scope, pixels): (&Scope, usize),
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
            .charge_product(repairs.len(), size_of::<Diagnostic>())?;
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
            region: bounds,
        };
        let prepared = Source::prepare(
            paint,
            &context,
            pixels,
            (&mut budget.functions, &budget.bytes),
        )?;
        self.add_paint(prepared, (scope, pixels), budget)
    }
}
