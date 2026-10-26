//! Prepared paint of marks: solids once per paint, state and target,
//! shadings and images per mark, stencil masks with their solid colour,
//! each reserved with its bytes, geometry work and repairs.

use super::{Colour, is_pattern};
use crate::plan::area;
use crate::prepare::Budget;
use crate::prepare::index;
use crate::prepare::planner::{ImageKey, Planner, Scope, SolidKey};
use crate::source::{ImageBudgets, PaintContext, ScratchLayout, Source};
use pagura_color::Painted;
use pagura_core::{Error, Result};
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
        let mut image = None;
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
                let index = self.add_paint(prepared, pixels, None, budget)?;
                if matches!(*paint, Paint::Solid(_)) {
                    budget.bytes.reserve_map_capacity(self.solids(), 1)?;
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
                image = Some(ImageKey {
                    image: Arc::as_ptr(&node.image).addr(),
                    state: state_id,
                    layout: scope.layout,
                    colorants: target.colorants,
                });
                let stencil = match (&node.image.space, node.stencil) {
                    (Some(_), _) => None,
                    (None, stencil) => Some(self.stencil(scope, stencil, &context, budget)?),
                };
                let prepared = Source::image(
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
                );
                let released = stencil
                    .as_ref()
                    .map_or(0, |(color, _)| size_of_val(&**color));
                drop(stencil);
                budget.bytes.refund(released);
                prepared?
            }
        };
        self.add_paint(prepared, pixels, image, budget)
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
        let (color, painted) =
            Source::solid_color(color, context, (&mut budget.functions, &budget.bytes))?;
        budget.bytes.charge(size_of_val(&*color))?;
        Ok((color, painted))
    }

    /// Keeps a prepared paint, reserving its bytes and its geometry work
    /// over `pixels` (mesh shadings), the repairs preparing it made and its
    /// execution storage, shared by the draws of `image`.
    fn add_paint(
        &mut self,
        paint: Source,
        pixels: usize,
        image: Option<ImageKey>,
        budget: &mut Budget,
    ) -> Result<u32> {
        let index = u32::try_from(self.parts.paints.len())
            .map_err(|_| Error::limit("paint count exceeds u32"))?;
        let payload = paint.bytes();
        budget.bytes.charge(payload)?;
        let layout = paint.scratch_layout();
        let retained = (|| {
            budget.work.charge_product(pixels, paint.work_per_pixel())?;
            budget.bytes.reserve_capacity(&mut self.parts.paints, 1)?;
            budget.bytes.reserve_capacity(&mut self.parts.slots.of, 1)?;
            budget
                .bytes
                .reserve_capacity(&mut self.repairs, paint.repairs().len())?;
            self.slot(layout, image, budget)
        })();
        let slot = match retained {
            Ok(slot) => slot,
            Err(error) => {
                drop(paint);
                budget.bytes.refund(payload);
                return Err(error);
            }
        };
        self.repairs.extend_from_slice(paint.repairs());
        self.parts.paints.push(paint);
        self.parts.slots.of.push(slot);
        Ok(index)
    }

    /// The storage of a paint with `layout`: the empty one without storage,
    /// the storage of the latest draw of `image` where it has the same
    /// layout, and a new one otherwise.
    fn slot(
        &mut self,
        layout: Option<ScratchLayout>,
        image: Option<ImageKey>,
        budget: &mut Budget,
    ) -> Result<u32> {
        let slots = &mut self.parts.slots;
        if layout.is_none()
            && let Some(empty) = slots.empty
        {
            return Ok(empty);
        }
        if let Some(key) = image
            && let Some(&slot) = self.images.get(&key)
            && layout.is_some()
            && slots.layouts.get(index(slot)) == Some(&layout)
        {
            return Ok(slot);
        }
        let slot = u32::try_from(slots.layouts.len())
            .map_err(|_| Error::limit("paint storage count exceeds u32"))?;
        budget.bytes.reserve_capacity(&mut slots.layouts, 1)?;
        slots.layouts.push(layout);
        match (layout, image) {
            (None, _) => slots.empty = Some(slot),
            (Some(_), Some(key)) => {
                budget.bytes.reserve_map_capacity(&mut self.images, 1)?;
                self.images.insert(key, slot);
            }
            (Some(_), None) => {}
        }
        Ok(slot)
    }

    /// The storage of paint `paint`.
    pub(super) fn scratch_of(&self, paint: u32) -> u32 {
        self.parts.slots.of.get(index(paint)).copied().unwrap_or(0)
    }

    /// A solid paint for an uncoloured pattern's cell content.
    pub(crate) fn solid_paint(
        &mut self,
        paint: &Paint,
        (state, scope): (&Composite, &Scope),
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
        self.add_paint(prepared, pixels, None, budget)
    }
}
