//! Soft masks (11.6.5): planned at their first use for the user's transform
//! and blending space, and reserved over the pixels that use them.

use super::{MaskKey, Planner, Scope};
use crate::format::Format;
use crate::plan::{Curve, GroupPlan, Layout, MaskPlan, Start, area, intersect, union};
use crate::prepare::{Budget, bounds, index};
use crate::process;
use pagura_color::Process;
use pagura_core::{Error, Result};
use pagura_interpret::{MaskId, MaskKind};
use pagura_raster::DeviceRect;

impl Planner<'_> {
    /// Records that `bounds` uses mask `mask`, and reserves its values
    /// there and its `TR` evaluations.
    pub(crate) fn use_mask(
        &mut self,
        mask: Option<u32>,
        bounds: DeviceRect,
        budget: &mut Budget,
    ) -> Result<()> {
        let Some(mask) = mask else {
            return Ok(());
        };
        if let Some(users) = self.users.get_mut(index(mask)) {
            *users = union(*users, bounds);
        }
        let cost = self
            .parts
            .masks
            .get(index(mask))
            .and_then(|m| m.transfer.as_ref())
            .map_or(0, Curve::cost);
        // The mask's buffer starts and becomes values over these pixels.
        budget.work.charge_product(area(bounds), 2)?;
        budget.functions.charge_product(area(bounds), cost)
    }

    /// The mask plan of soft mask `id` used in `scope`, prepared at its
    /// first use with the scope's transform and blending space (11.6.5).
    pub(crate) fn mask_use(
        &mut self,
        scope: &Scope,
        id: MaskId,
        budget: &mut Budget,
    ) -> Result<u32> {
        let key = MaskKey {
            mask: id,
            transform: scope.transform_bits(),
            process: scope.layout.process,
        };
        if let Some(&plan) = self.masks.get(&key) {
            return Ok(plan);
        }
        if self.open.contains(&key) {
            return Err(Error::syntax("soft mask content uses the soft mask"));
        }
        let scene = self.scene;
        let mask = scene
            .masks
            .get(index(id.0))
            .ok_or(Error::invalid_argument("scene mask out of range"))?;
        let group = scene
            .group(mask.group)
            .ok_or(Error::invalid_argument("scene group out of range"))?;
        let (format, upright) = self
            .storage
            .masks
            .get(&(id, scope.layout.process))
            .copied()
            .unwrap_or((Format::F64, 0.0));
        let layout = Layout {
            process: group.attributes.space.unwrap_or(scope.layout.process),
            spots: 0,
        };
        let (backdrop, transfer) = mask_parameters(mask, layout.process)?;
        let isolated = group.attributes.isolated;
        let start = if mask.kind == MaskKind::Luminosity && !isolated {
            Start::Opaque(backdrop)
        } else {
            Start::Transparent
        };
        let hull = group
            .bbox
            .as_ref()
            .map_or(Some([f64::MIN, f64::MIN, f64::MAX, f64::MAX]), |b| {
                bounds::path(b, scope.transform).map(|h| bounds::expand(h, 1.0))
            });
        let culling = bounds::device(hull, self.area.region);
        let child = self.new_group(
            GroupPlan {
                bounds: culling,
                items: Vec::new(),
                order: Vec::new(),
                layout,
                format,
                knockout: group.attributes.knockout,
                start,
            },
            budget,
        )?;
        let inner = Scope {
            group: child,
            transform: scope.transform,
            crop: false,
            bounds: culling,
            layout,
            plain: false,
            records: false,
            separated: false,
            masked: true,
            paint: None,
            depth: scope.depth + 1,
            upright,
        };
        budget.bytes.reserve_capacity(&mut self.open, 1)?;
        self.open.push(key);
        let walked = self.children(&inner, &group.children, budget);
        self.open.pop();
        walked?;
        let content = intersect(self.content(child), culling);
        if let Some(plan) = self.parts.groups.get_mut(index(child)) {
            plan.bounds = content;
        }
        let value = match mask.kind {
            MaskKind::Alpha => 0.0,
            MaskKind::Luminosity => process::luminosity(layout.process, &backdrop),
        }
        .clamp(0.0, 1.0);
        let mut plan = MaskPlan {
            group: child,
            kind: mask.kind,
            isolated,
            backdrop,
            transfer,
            extent: content,
            outside: 0.0,
        };
        plan.outside = crate::execute::mask_transfer(&plan, value);
        self.add_mask(key, plan, budget)
    }

    /// Retains one plan and its use bounds, each in charged containers.
    fn add_mask(&mut self, key: MaskKey, plan: MaskPlan, budget: &mut Budget) -> Result<u32> {
        let id = u32::try_from(self.parts.masks.len())
            .map_err(|_| Error::limit("soft mask count exceeds u32"))?;
        budget.bytes.reserve_capacity(&mut self.parts.masks, 1)?;
        budget.bytes.reserve_capacity(&mut self.users, 1)?;
        budget.bytes.reserve_map_capacity(&mut self.masks, 1)?;
        self.parts.masks.push(plan);
        self.users.push(DeviceRect::default());
        self.masks.insert(key, id);
        Ok(id)
    }
}

/// A soft mask's `BC` and `TR`.
type MaskParameters = ([f64; 4], Option<Curve>);

/// A soft mask's `BC` in its group's blending space, additively, and its
/// `TR`.
fn mask_parameters(mask: &pagura_interpret::SoftMask, process: Process) -> Result<MaskParameters> {
    let backdrop = match &mask.backdrop {
        Some(components) => {
            if components.len() != process.components() {
                return Err(Error::syntax(
                    "soft-mask backdrop has the wrong component count",
                ));
            }
            process::native(process, components)
        }
        None => process::black(process),
    };
    let transfer = mask.transfer.as_ref().map(Curve::new).transpose()?;
    Ok((backdrop, transfer))
}
