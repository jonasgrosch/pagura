//! The walk of preparation: the scene tree in source order. Marks are
//! prepared in `marks` (glyph runs gathered in `runs`), clip states in
//! `clips`, transparency groups and their buffers in `groups`, soft masks
//! at their first use in `masks` and tiling patterns by instance in
//! `patterns`.

mod clips;
mod groups;
mod marks;
mod masks;
mod patterns;
pub(crate) mod runs;

pub(crate) use groups::view_transform;

use super::storage::{self, Inputs, Storage};
use super::{Budget, ShapeBuilder, index, overprint};
use crate::plan::{
    Conversion, Curve, GroupPlan, Item, Layout, MaskPlan, Start, TargetNames, union,
};
use crate::request::RenderRequest;
use crate::source::Source;
use pagura_color::{Process, Target};
use pagura_core::hash::FastMap;
use pagura_core::{Diagnostic, Error, Limits, Meter, Name, Position, Result, Store};
use pagura_interpret::{MaskId, Node, NodeId, Paint, Scene};
use pagura_raster::{Area, DeviceRect, Path, Transform};
use std::sync::Arc;

/// A prepared solid paint's identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct SolidKey {
    /// The scene paint.
    pub(crate) paint: u32,
    /// The composite state.
    pub(crate) state: u32,
    /// The buffer's channels.
    pub(crate) layout: Layout,
    /// How the target takes colorants.
    pub(crate) colorants: pagura_color::Colorants,
}

/// A soft mask planned for one transform and parent blending space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct MaskKey {
    /// The scene mask.
    pub(crate) mask: MaskId,
    /// The bits of the content transform.
    pub(crate) transform: [u64; 6],
    /// The parent's blending space.
    pub(crate) process: Process,
}

/// The state of the walk.
pub(crate) struct Planner<'s> {
    pub(crate) scene: &'s Scene,
    pub(crate) request: &'s RenderRequest,
    pub(crate) store: &'s Store,
    pub(crate) limits: &'s Limits,
    pub(crate) shapes: ShapeBuilder<'s>,
    pub(crate) area: Area,
    pub(crate) storage: Storage,
    /// Held until the ordered walk's transfer check has dropped its scratch.
    pub(crate) transfer_bytes: usize,
    pub(crate) simulate: bool,
    /// Spot colorants of the page target.
    pub(crate) names: TargetNames,
    /// Spot colorants with their first definition.
    pub(crate) definitions: Vec<(Name, Arc<pagura_color::ColorSpace>)>,
    pub(crate) parts: Parts,
    /// The page crop in default user space.
    pub(crate) crop: Arc<Path>,
    clips: FastMap<clips::Key, clips::State>,
    /// Prepared clips by their paths, which clip states share.
    built_clips: clips::Built,
    /// The clip state looked up last: marks in a row usually share one.
    last_clip: Option<(clips::Key, clips::State)>,
    solids: FastMap<SolidKey, u32>,
    masks: FastMap<MaskKey, u32>,
    /// Masks whose content is being walked.
    open: Vec<MaskKey>,
    /// Union of the bounds that use each mask plan.
    pub(crate) users: Vec<DeviceRect>,
    transfers: FastMap<u32, u16>,
    /// The serial of the mark being prepared.
    pub(crate) serial: u32,
    /// Where the walk failed.
    pub(crate) failed: Option<(u32, Position)>,
    /// Repairs made preparing paint, in source order.
    pub(crate) repairs: Vec<Diagnostic>,
    /// The group whose glyph run's placements are being gathered; content
    /// walked meanwhile (a soft mask's, at its first use) is not.
    gathering: Option<u32>,
    /// The run being gathered.
    run: Option<runs::Open>,
    /// Placements gathered into runs, for the mark budget.
    gathered: usize,
    /// What the placements of the glyph run being gathered share, once one
    /// of them has been prepared in full.
    memo: Option<runs::Memo>,
}

/// What the walk builds.
#[derive(Debug, Default)]
pub(crate) struct Parts {
    pub(crate) groups: Vec<GroupPlan>,
    pub(crate) items: Vec<Item>,
    pub(crate) masks: Vec<MaskPlan>,
    pub(crate) paints: Vec<Source>,
    pub(crate) conversions: Vec<Conversion>,
    pub(crate) transfers: Vec<[Option<Curve>; 3]>,
}

/// Where content is planned.
#[derive(Debug, Clone)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent properties of the content's place in the scene"
)]
pub(crate) struct Scope {
    /// The group plan that receives items.
    pub(crate) group: u32,
    /// Content space to device pixels.
    pub(crate) transform: Transform,
    /// Whether the page crop is the outermost clip: page content only.
    pub(crate) crop: bool,
    /// Pixels items may touch.
    pub(crate) bounds: DeviceRect,
    /// The receiving buffer's channels.
    pub(crate) layout: Layout,
    /// Every enclosing group composites plainly, so marks may select
    /// transfer functions (11.7.5.2).
    pub(crate) plain: bool,
    /// Page content, which records transfer selections.
    pub(crate) records: bool,
    /// Colours keep spot colorants as channels (overprint simulation);
    /// soft masks use the alternates.
    pub(crate) separated: bool,
    /// Soft-mask content, which the transfer gate does not reach.
    pub(crate) masked: bool,
    /// The paint of an uncoloured tiling pattern's cell content: its index
    /// and the paint, for stencil masks that need their own.
    pub(crate) paint: Option<(u32, Arc<Paint>)>,
    /// Nesting.
    pub(crate) depth: usize,
}

impl<'s> Planner<'s> {
    /// Decides the page's separations and storage and readies the walk.
    ///
    /// # Errors
    ///
    /// The errors of the whole-content survey, which do not depend on the
    /// region.
    pub(crate) fn new(
        scene: &'s Scene,
        request: &'s RenderRequest,
        (store, limits): (&'s Store, &'s Limits),
        area: Area,
        shapes: ShapeBuilder<'s>,
        budget: &mut Budget,
    ) -> Result<Self> {
        let separations =
            overprint::discover(scene, request.overprint, limits.depth, &mut budget.bytes)?;
        let root = scene
            .group(scene.root)
            .ok_or(Error::invalid_argument("scene group out of range"))?;
        let default = if separations.simulate {
            Process::Cmyk
        } else {
            Process::Rgb
        };
        let page = Layout {
            process: root.attributes.space.unwrap_or(default),
            spots: separations.spots.len(),
        };
        let storage = storage::survey(
            scene,
            Inputs {
                page,
                simulate: separations.simulate,
                policy: request.overprint == crate::request::OverprintMode::Simulate,
                depth: limits.depth,
                transform: request.transform,
                pixels: request.page,
                user_unit: scene.page.user_unit,
            },
            &mut budget.bytes,
        )?;
        let transfer_bytes = if storage.transfers {
            super::finish::reserve_transfer_check(area, &mut budget.bytes)?
        } else {
            0
        };
        let names = target_names(&separations, &mut budget.bytes)?;
        let mut parts = Parts::default();
        budget.bytes.reserve_capacity(&mut parts.groups, 1)?;
        parts.groups.push(GroupPlan {
            bounds: area.region,
            items: Vec::new(),
            order: Vec::new(),
            layout: page,
            format: storage.page,
            knockout: storage.page_knockout,
            start: Start::Transparent,
        });
        budget
            .bytes
            .charge(2 * size_of::<usize>() + size_of::<Path>())?;
        let crop = Arc::new(Path::rect_with_bytes(scene.page.crop, &mut budget.bytes)?);
        Ok(Self {
            scene,
            request,
            store,
            limits,
            shapes,
            area,
            storage,
            transfer_bytes,
            simulate: separations.simulate,
            names,
            definitions: separations.spots,
            parts,
            crop,
            clips: FastMap::default(),
            built_clips: FastMap::default(),
            last_clip: None,
            solids: FastMap::default(),
            masks: FastMap::default(),
            open: Vec::new(),
            users: Vec::new(),
            transfers: FastMap::default(),
            serial: 0,
            failed: None,
            repairs: Vec::new(),
            gathering: None,
            run: None,
            gathered: 0,
            memo: None,
        })
    }

    /// Prepares the page's content.
    ///
    /// # Errors
    ///
    /// The first error in source order among the marks that meet the
    /// region; its serial and position are left in `failed`.
    pub(crate) fn page(&mut self, budget: &mut Budget) -> Result<()> {
        let scene = self.scene;
        let root = scene
            .group(scene.root)
            .ok_or(Error::invalid_argument("scene group out of range"))?;
        let page = self.parts.groups.first().map(|g| g.layout);
        let scope = Scope {
            group: 0,
            transform: self.request.transform,
            crop: true,
            bounds: self.area.region,
            layout: page.unwrap_or(Layout {
                process: Process::Rgb,
                spots: 0,
            }),
            plain: true,
            records: self.storage.transfers,
            separated: self.simulate,
            masked: false,
            paint: None,
            depth: 0,
        };
        self.children(&scope, &root.children, budget)
    }

    /// The target of colours painted in `scope`: separated under overprint
    /// simulation, composite otherwise and in soft masks.
    pub(crate) fn target(&self, scope: &Scope) -> Target {
        if scope.separated {
            let spots = if scope.layout.spots > 0 {
                Arc::clone(&self.names.spots)
            } else {
                Arc::clone(&self.names.empty)
            };
            Target::separated(scope.layout.process, spots)
        } else if self.request.overprint == crate::request::OverprintMode::Simulate {
            // Process colours still overprint where nothing is separated, in
            // soft masks in particular.
            Target {
                colorants: pagura_color::Colorants::ProcessOverprint,
                process: scope.layout.process,
                spots: Arc::clone(&self.names.empty),
            }
        } else {
            Target {
                process: scope.layout.process,
                spots: Arc::clone(&self.names.empty),
                colorants: pagura_color::Colorants::Composite,
            }
        }
    }

    fn children(&mut self, scope: &Scope, children: &[NodeId], budget: &mut Budget) -> Result<()> {
        if scope.depth >= self.limits.depth {
            return Err(Error::limit("compositing depth exceeded"));
        }
        let scene = self.scene;
        for &id in children {
            let node = scene
                .node(id)
                .ok_or(Error::invalid_argument("scene node out of range"))?;
            match node {
                Node::Fill(fill) => self.fill(scope, fill, budget)?,
                Node::Stroke(stroke) => self.stroke(scope, stroke, budget)?,
                Node::Glyphs(glyphs) => self.glyphs(scope, glyphs, budget)?,
                Node::Image(image) => self.image(scope, image, budget)?,
                Node::Shading(shading) => self.shading(scope, shading, budget)?,
                Node::Group(group) => self.group(scope, *group, budget)?,
            }
        }
        Ok(())
    }

    /// Adds `item` to `group`.
    pub(crate) fn push(&mut self, group: u32, item: Item, budget: &mut Budget) -> Result<()> {
        // A run being gathered comes first in source order.
        self.flush(budget)?;
        if self.parts.items.len().saturating_add(self.gathered) >= self.limits.display_commands {
            return Err(Error::limit("painted mark budget exceeded"));
        }
        let id = u32::try_from(self.parts.items.len())
            .map_err(|_| Error::limit("painted mark budget exceeded"))?;
        budget.bytes.reserve_capacity(&mut self.parts.items, 1)?;
        if let Some(group) = self.parts.groups.get_mut(index(group)) {
            budget.bytes.reserve_capacity(&mut group.items, 1)?;
            // Finalization constructs the one order slot for this item.
            budget.bytes.charge_items::<u32>(1)?;
        }
        self.parts.items.push(item);
        if let Some(group) = self.parts.groups.get_mut(index(group)) {
            group.items.push(id);
        }
        Ok(())
    }

    /// The first and last band of `bounds`.
    pub(crate) fn bands(&self, bounds: DeviceRect) -> [u32; 2] {
        let height = self.area.band_height.max(1);
        [bounds.y0 / height, bounds.y1.saturating_sub(1) / height]
    }

    /// The union of the bounds of `group`'s items.
    fn content(&self, group: u32) -> DeviceRect {
        self.parts
            .groups
            .get(index(group))
            .map_or_else(DeviceRect::default, |g| {
                g.items
                    .iter()
                    .filter_map(|&id| self.parts.items.get(index(id)))
                    .fold(DeviceRect::default(), |acc, item| union(acc, item.bounds))
            })
    }

    fn new_group(&mut self, plan: GroupPlan, budget: &mut Budget) -> Result<u32> {
        let id = u32::try_from(self.parts.groups.len())
            .map_err(|_| Error::limit("group count exceeds u32"))?;
        budget.bytes.reserve_capacity(&mut self.parts.groups, 1)?;
        self.parts.groups.push(plan);
        Ok(id)
    }

    /// Records a failure at the current mark.
    pub(crate) fn fail(&mut self, error: Error, position: Position) -> Error {
        if self.failed.is_none() {
            self.failed = Some((self.serial, position));
        }
        error.at(position).in_object_opt(position.object())
    }

    /// The selection index of a state's transfer functions.
    pub(crate) fn transfer(
        &mut self,
        state: u32,
        transfer: &pagura_interpret::Transfer,
        budget: &mut Budget,
    ) -> Result<u16> {
        if let Some(&index) = self.transfers.get(&state) {
            return Ok(index);
        }
        let functions = match transfer {
            pagura_interpret::Transfer::All(f) => {
                let f = Curve::new(f)?;
                [Some(f.clone()), Some(f.clone()), Some(f)]
            }
            pagura_interpret::Transfer::PerComponent(functions) => {
                let mut out: [Option<Curve>; 3] = [None, None, None];
                for (slot, f) in out.iter_mut().zip(functions) {
                    *slot = f.as_ref().map(Curve::new).transpose()?;
                }
                out
            }
        };
        budget
            .bytes
            .reserve_capacity(&mut self.parts.transfers, 1)?;
        budget.bytes.reserve_map_capacity(&mut self.transfers, 1)?;
        self.parts.transfers.push(functions);
        let index = u16::try_from(self.parts.transfers.len())
            .map_err(|_| Error::limit("transfer function count exceeds u16"))?;
        self.transfers.insert(state, index);
        Ok(index)
    }

    pub(crate) fn solids(&mut self) -> &mut FastMap<SolidKey, u32> {
        &mut self.solids
    }

    pub(crate) fn clip_states(&mut self) -> &mut FastMap<clips::Key, clips::State> {
        &mut self.clips
    }

    /// Drops the walk's setup tables and temporary owners before the
    /// workspace is reserved. The result retains plan data and the owners
    /// still needed for post-walk transfer checking and spot preparation.
    pub(crate) fn finish(self, bytes: &mut Meter) -> Result<Walked<'s>> {
        let released = self.storage.retained_bytes(bytes)?
            + 2 * size_of::<usize>()
            + size_of::<Path>()
            + self.crop.retained_bytes()
            + bytes.map_bytes(&self.clips)?
            + clips::retained_bytes(&self.built_clips, bytes)?
            + bytes.map_bytes(&self.solids)?
            + bytes.map_bytes(&self.masks)?
            + bytes.map_bytes(&self.transfers)?
            + self.open.capacity() * size_of::<MaskKey>()
            + self.users.capacity() * size_of::<DeviceRect>();
        let Self {
            shapes,
            names,
            parts,
            repairs,
            storage,
            definitions,
            transfer_bytes,
            crop,
            clips,
            built_clips,
            solids,
            masks,
            transfers,
            open,
            users,
            ..
        } = self;
        drop((
            storage,
            crop,
            clips,
            built_clips,
            solids,
            masks,
            transfers,
            open,
            users,
        ));
        bytes.refund(released);
        Ok(Walked {
            shapes,
            names,
            parts,
            repairs,
            definitions,
            transfer_bytes,
        })
    }
}

/// The retained result of the ordered walk.
pub(crate) struct Walked<'s> {
    pub(crate) shapes: ShapeBuilder<'s>,
    pub(crate) names: TargetNames,
    pub(crate) parts: Parts,
    pub(crate) repairs: Vec<Diagnostic>,
    pub(crate) definitions: Vec<(Name, Arc<pagura_color::ColorSpace>)>,
    pub(crate) transfer_bytes: usize,
}

/// Copies only the owned name bytes, then moves those names into one
/// shared payload while both its allocation and the old slots are charged.
fn target_names(separations: &overprint::Separations, bytes: &mut Meter) -> Result<TargetNames> {
    bytes.charge_items::<usize>(2)?;
    let empty: Arc<[Name]> = Arc::new([]);
    if separations.spots.is_empty() {
        return Ok(TargetNames {
            spots: Arc::clone(&empty),
            empty,
        });
    }
    let mut names = Vec::new();
    bytes.reserve_capacity(&mut names, separations.spots.len())?;
    for (name, _) in &separations.spots {
        bytes.charge(name.heap_bytes())?;
        names.push(name.clone());
    }
    bytes.charge_items::<usize>(2)?;
    bytes.charge_items::<Name>(names.len())?;
    let released = names.capacity() * size_of::<Name>();
    let spots = names.into();
    bytes.refund(released);
    Ok(TargetNames { spots, empty })
}

impl Scope {
    /// The bits of the transform, as a key.
    pub(crate) fn transform_bits(&self) -> [u64; 6] {
        let t = self.transform;
        [t.a, t.b, t.c, t.d, t.e, t.f].map(f64::to_bits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pagura_color::ColorSpace;

    #[test]
    fn shared_target_names_charge_owned_copy_overlap_then_only_live_owners() {
        let definitions = overprint::Separations {
            simulate: true,
            spots: vec![
                (
                    Name::new(b"An owned spot name longer than the inline limit"),
                    Arc::new(ColorSpace::DeviceRgb),
                ),
                (Name::new(b"Other"), Arc::new(ColorSpace::DeviceRgb)),
            ],
        };
        let heap = definitions
            .spots
            .iter()
            .map(|(name, _)| name.heap_bytes())
            .sum::<usize>();
        let slots = definitions.spots.len() * size_of::<Name>();
        let peak = 4 * size_of::<usize>() + 2 * slots + heap;
        let mut tiny = Meter::new(peak - 1, "name memory");
        assert_eq!(
            target_names(&definitions, &mut tiny).unwrap_err().message,
            "name memory"
        );
        let mut bytes = Meter::new(peak, "name memory");
        let names = target_names(&definitions, &mut bytes).unwrap();
        assert_eq!(bytes.used(), 4 * size_of::<usize>() + slots + heap);
        assert_eq!(names.spots.len(), 2);
        assert!(!Arc::ptr_eq(&names.spots, &names.empty));

        let mut empty_bytes = Meter::new(2 * size_of::<usize>(), "name memory");
        let names = target_names(&overprint::Separations::default(), &mut empty_bytes).unwrap();
        assert!(Arc::ptr_eq(&names.spots, &names.empty));
        assert_eq!(empty_bytes.used(), 2 * size_of::<usize>());
    }
}
