//! Transparency groups (11.4): flattened into their parent or planned as a
//! buffer of their own whose result composites into the parent, converted
//! into its blending space where they differ; fixed views (12.5.3).

use super::{Planner, Scope};
use crate::plan::{
    Compositing, Conversion, Curve, GroupPlan, Item, ItemKind, Layout, Start, area, intersect,
};
use crate::prepare::storage::{self, Route};
use crate::prepare::{Budget, bounds, index};
use pagura_color::{Encoder, Process};
use pagura_core::{Error, Position, Result};
use pagura_interpret::{GroupId, GroupRole};
use pagura_raster::{DeviceRect, Transform};
use std::sync::Arc;

impl Planner<'_> {
    /// A transparency group (11.4): its members composite into the parent
    /// directly where the survey flattened it, or into a buffer of its own.
    pub(super) fn group(&mut self, scope: &Scope, id: GroupId, budget: &mut Budget) -> Result<()> {
        let scene = self.scene;
        let group = scene
            .group(id)
            .ok_or(Error::invalid_argument("scene group out of range"))?;
        if group.role != GroupRole::Transparency {
            return Ok(());
        }
        let decision = *self.storage.groups.get(&(id, scope.layout.process)).ok_or(
            Error::invalid_argument("scene group outside the content the storage survey walked"),
        )?;
        let state = Arc::clone(
            scene
                .states
                .get(group.mark.state.index())
                .ok_or(Error::invalid_argument("scene state out of range"))?,
        );
        let transform = match group.view {
            Some(view) => view_transform(scope.transform, view, scene.page.user_unit)?,
            None => scope.transform,
        };
        let hull = group
            .bbox
            .as_ref()
            .map_or(Some([f64::MIN, f64::MIN, f64::MAX, f64::MAX]), |b| {
                bounds::path(b, transform).map(|h| bounds::expand(h, 1.0))
            });
        let clip = self.clip_bounds(scope, group.mark.clip, None)?;
        let culling = intersect(
            intersect(bounds::device(hull, self.area.region), clip),
            scope.bounds,
        );
        if culling.is_empty() {
            return Ok(());
        }
        let plain = state.alpha == 1.0
            && state.blend == pagura_interpret::BlendMode::Normal
            && state.soft_mask.is_none();
        let inner = |group: u32, layout: Layout| Scope {
            group,
            transform,
            crop: scope.crop,
            bounds: culling,
            layout,
            plain: scope.plain && plain,
            records: scope.records,
            separated: scope.separated,
            masked: scope.masked,
            paint: scope.paint.clone(),
            depth: scope.depth + 1,
        };
        match decision.route {
            Route::Skip => return Ok(()),
            Route::Flatten => {
                return self.children(&inner(scope.group, scope.layout), &group.children, budget);
            }
            Route::Buffer => {}
        }
        self.serial += 1;
        let serial = self.serial;
        let position = group.mark.position;
        let mask = state
            .soft_mask
            .map(|mask| self.mask_use(scope, mask, budget))
            .transpose()
            .map_err(|e| self.fail(e, position))?;
        let layout = storage::group_layout(group.attributes, scope.layout);
        let child = self.new_group(
            GroupPlan {
                bounds: culling,
                items: Vec::new(),
                order: Vec::new(),
                layout,
                format: decision.format,
                knockout: decision.knockout,
                start: if decision.transparent {
                    Start::Transparent
                } else {
                    Start::Backdrop
                },
            },
            budget,
        )?;
        self.children(&inner(child, layout), &group.children, budget)?;
        let result = GroupResult {
            group: child,
            state: &state,
            culling,
            serial,
            position,
            mask,
        };
        self.group_result(scope, &result, budget)
    }

    /// Composites a planned group's result into `scope`'s group.
    fn group_result(
        &mut self,
        scope: &Scope,
        result: &GroupResult<'_>,
        budget: &mut Budget,
    ) -> Result<()> {
        let child = result.group;
        let bounds = intersect(self.content(child), result.culling);
        if bounds.is_empty() {
            return Ok(());
        }
        let layout = match self.parts.groups.get_mut(index(child)) {
            Some(plan) => {
                plan.bounds = bounds;
                plan.layout
            }
            None => return Ok(()),
        };
        let state = result.state;
        let conversion = if layout.process == scope.layout.process {
            None
        } else {
            let to = scope.layout.process;
            Some(self.conversion(layout.process, to, &state.conversion, bounds, budget)?)
        };
        let compositing = Compositing {
            alpha: state.alpha,
            blend: state.blend,
            alpha_is_shape: state.alpha_is_shape,
            mask: result.mask,
            overprint: 0,
        };
        self.use_mask(result.mask, bounds, budget)?;
        budget.work.charge_product(area(bounds), 3)?;
        let item = Item {
            bounds,
            bands: self.bands(bounds),
            serial: result.serial,
            position: result.position,
            work: 0,
            kind: ItemKind::Group(crate::plan::GroupUse {
                group: child,
                cover: None,
                compositing,
                conversion,
            }),
        };
        self.push(scope.group, item, budget)
    }

    /// A conversion of group results from `from` to `to` with the group's
    /// black generation and undercolour removal.
    fn conversion(
        &mut self,
        from: Process,
        to: Process,
        conversion: &pagura_color::Conversion,
        bounds: DeviceRect,
        budget: &mut Budget,
    ) -> Result<u32> {
        // BG and UCR map one input, as transfer functions do.
        for function in [&conversion.black_generation, &conversion.undercolor_removal]
            .into_iter()
            .flatten()
        {
            Curve::new(function)?;
        }
        let encoder = Encoder::new(to, conversion);
        budget
            .functions
            .charge_product(area(bounds), encoder.cost())?;
        budget
            .bytes
            .reserve_capacity(&mut self.parts.conversions, 1)?;
        let id = u32::try_from(self.parts.conversions.len())
            .map_err(|_| Error::limit("conversion count exceeds u32"))?;
        self.parts
            .conversions
            .push(Conversion { from, to, encoder });
        Ok(id)
    }
}

/// A planned group whose result is composited into its parent.
struct GroupResult<'a> {
    group: u32,
    state: &'a pagura_interpret::Composite,
    culling: DeviceRect,
    serial: u32,
    position: Position,
    mask: Option<u32>,
}

/// The device transform of a fixed view (12.5.3): `NoRotate` undoes the
/// page's rotation and `NoZoom` its scale, about the anchor.
pub(crate) fn view_transform(
    device: Transform,
    view: pagura_interpret::ViewFit,
    user_unit: f64,
) -> Result<Transform> {
    let anchor = device.apply(view.anchor);
    let angle = if view.no_rotate {
        -device.b.atan2(device.a)
    } else {
        0.0
    };
    let scale = if view.no_zoom {
        user_unit / device.a.hypot(device.b)
    } else {
        1.0
    };
    let (sin, cos) = angle.sin_cos();
    let transform = device
        .then(Transform::translate(-anchor.x, -anchor.y))
        .then(Transform::new(
            scale * cos,
            scale * sin,
            -scale * sin,
            scale * cos,
            anchor.x,
            anchor.y,
        ));
    if transform.is_finite() {
        Ok(transform)
    } else {
        Err(Error::limit("annotation view transform overflow"))
    }
}
