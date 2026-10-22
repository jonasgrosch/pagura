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
use crate::hash::{self, FastMap};
use crate::plan::{Conversion, Curve, GroupPlan, Item, Layout, MaskPlan, Start, union};
use crate::request::RenderRequest;
use crate::source::Source;
use pagura_color::{Process, Target};
use pagura_core::{Error, Limits, Name, Position, Result, Store};
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

/// Bytes charged per item: the item and its places in its group's lists.
const ITEM_BYTES: usize = size_of::<Item>() + 2 * size_of::<u32>();

/// The state of the walk.
pub(crate) struct Planner<'s> {
    pub(crate) scene: &'s Scene,
    pub(crate) request: &'s RenderRequest,
    pub(crate) store: &'s Store,
    pub(crate) limits: &'s Limits,
    pub(crate) shapes: ShapeBuilder<'s>,
    pub(crate) area: Area,
    pub(crate) storage: Storage,
    pub(crate) simulate: bool,
    /// Spot colorants of the page target.
    pub(crate) spots: Arc<[Name]>,
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
    pub(crate) repairs: Vec<pagura_core::Diagnostic>,
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
    /// The edges handle of each source of the glyph node being prepared.
    glyph_edges: Vec<Option<u32>>,
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
    ) -> Result<Self> {
        let separations = overprint::discover(scene, request.overprint, limits.depth)?;
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
        )?;
        let spots: Arc<[Name]> = separations
            .spots
            .iter()
            .map(|(name, _)| name.clone())
            .collect();
        let mut parts = Parts::default();
        parts.groups.push(GroupPlan {
            bounds: area.region,
            items: Vec::new(),
            order: Vec::new(),
            layout: page,
            format: storage.page,
            knockout: storage.page_knockout,
            start: Start::Transparent,
        });
        Ok(Self {
            scene,
            request,
            store,
            limits,
            shapes,
            area,
            storage,
            simulate: separations.simulate,
            spots,
            definitions: separations.spots,
            parts,
            crop: Arc::new(crop(scene)),
            clips: hash::map(),
            built_clips: hash::map(),
            last_clip: None,
            solids: hash::map(),
            masks: hash::map(),
            open: Vec::new(),
            users: Vec::new(),
            transfers: hash::map(),
            serial: 0,
            failed: None,
            repairs: Vec::new(),
            gathering: None,
            run: None,
            gathered: 0,
            memo: None,
            glyph_edges: Vec::new(),
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
        budget.bytes.charge(size_of::<GroupPlan>())?;
        self.children(&scope, &root.children, budget)
    }

    /// The target of colours painted in `scope`: separated under overprint
    /// simulation, composite otherwise and in soft masks.
    pub(crate) fn target(&self, scope: &Scope) -> Target {
        if scope.separated {
            let spots = if scope.layout.spots > 0 {
                Arc::clone(&self.spots)
            } else {
                Arc::from([])
            };
            Target::separated(scope.layout.process, spots)
        } else if self.request.overprint == crate::request::OverprintMode::Simulate {
            // Process colours still overprint where nothing is separated, in
            // soft masks in particular.
            Target {
                colorants: pagura_color::Colorants::ProcessOverprint,
                ..Target::new(scope.layout.process)
            }
        } else {
            Target::new(scope.layout.process)
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
        budget.bytes.charge(ITEM_BYTES)?;
        let id = u32::try_from(self.parts.items.len())
            .map_err(|_| Error::limit("painted mark budget exceeded"))?;
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
        budget.bytes.charge(size_of::<GroupPlan>())?;
        let id = u32::try_from(self.parts.groups.len())
            .map_err(|_| Error::limit("group count exceeds u32"))?;
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
        budget.bytes.charge(size_of::<[Option<Curve>; 3]>())?;
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
}

impl Scope {
    /// The bits of the transform, as a key.
    pub(crate) fn transform_bits(&self) -> [u64; 6] {
        let t = self.transform;
        [t.a, t.b, t.c, t.d, t.e, t.f].map(f64::to_bits)
    }
}

/// The page crop as a path, in default user space.
pub(crate) fn crop(scene: &Scene) -> Path {
    Path::rect(scene.page.crop)
}
