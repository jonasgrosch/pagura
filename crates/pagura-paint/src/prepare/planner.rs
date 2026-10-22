//! The walk of preparation: the scene tree in source order, groups and
//! their buffers, soft masks at their first use and tiling patterns by
//! instance. Marks are prepared in `marks`, clip states in `clips`.

mod clips;
mod marks;
pub(crate) mod runs;

use super::storage::{self, Inputs, Storage};
use super::{Backend, Budget, bounds, index, overprint};
use crate::execute::Tops;
use crate::format::Format;
use crate::hash::{self, FastMap};
use crate::paint::{Curve, PaintSpans};
use crate::plan::{
    Compositing, Conversion, GroupPlan, Item, ItemKind, Layout, MaskPlan, Start, area, intersect,
    union,
};
use crate::process;
use crate::request::RenderRequest;
use pagura_color::{Process, Target};
use pagura_core::{Error, Limits, Name, Position, Result, Store};
use pagura_interpret::{
    GroupId, GroupRole, MaskId, MaskKind, Node, NodeId, Paint, Scene, TilingColors, TilingPaint,
    TilingSpacing,
};
use pagura_raster::{Area, DeviceRect, Path, Precision, Transform};
use std::sync::Arc;

/// A prepared solid paint's identity: paint, state, layout and how the
/// target takes colorants.
pub(crate) type SolidKey = (u32, u32, Layout, pagura_color::Colorants);

/// Bytes charged per item: the item and its places in its group's lists.
const ITEM_BYTES: usize = size_of::<Item>() + 2 * size_of::<u32>();

/// The state of the walk.
pub(crate) struct Planner<'s, B: Backend> {
    pub(crate) scene: &'s Scene,
    pub(crate) request: &'s RenderRequest,
    pub(crate) store: &'s Store,
    pub(crate) limits: &'s Limits,
    pub(crate) backend: B,
    pub(crate) area: Area,
    pub(crate) precision: Precision,
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
    solids: FastMap<SolidKey, u32>,
    masks: FastMap<(MaskId, [u64; 6], Process), u32>,
    open: Vec<(MaskId, [u64; 6], Process)>,
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
}

/// What the walk builds.
#[derive(Debug, Default)]
pub(crate) struct Parts {
    pub(crate) groups: Vec<GroupPlan>,
    pub(crate) items: Vec<Item>,
    pub(crate) masks: Vec<MaskPlan>,
    pub(crate) paints: Vec<Box<dyn PaintSpans>>,
    pub(crate) conversions: Vec<Conversion>,
    pub(crate) transfers: Vec<[Option<Arc<dyn Curve>>; 3]>,
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

impl<'s, B: Backend> Planner<'s, B> {
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
        backend: B,
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
                bytes: request.fast.byte_storage,
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
        let precision = if request.fast.approximate_coverage {
            Precision::Approximate
        } else {
            Precision::Exact
        };
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
            backend,
            area,
            precision,
            storage,
            simulate: separations.simulate,
            spots,
            definitions: separations.spots,
            parts,
            crop: Arc::new(crop(scene)),
            clips: hash::map(),
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

    /// A transparency group (11.4): its members composite into the parent
    /// directly where the survey flattened it, or into a buffer of its own.
    fn group(&mut self, scope: &Scope, id: GroupId, budget: &mut Budget) -> Result<()> {
        let scene = self.scene;
        let group = scene
            .group(id)
            .ok_or(Error::invalid_argument("scene group out of range"))?;
        if group.role != GroupRole::Transparency {
            return Ok(());
        }
        let decision = *self
            .storage
            .groups
            .get(&(id, scope.layout.process))
            .ok_or(Error::invalid_argument("scene group visited twice"))?;
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
        if decision.flatten {
            return self.children(&inner(scope.group, scope.layout), &group.children, budget);
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
        let black = conversion
            .black_generation
            .as_ref()
            .map(|f| curve(f))
            .transpose()?;
        let removal = conversion
            .undercolor_removal
            .as_ref()
            .map(|f| curve(f))
            .transpose()?;
        let cost = black
            .as_ref()
            .map_or(0, |f| f.cost())
            .saturating_add(removal.as_ref().map_or(0, |f| f.cost()));
        if to == Process::Cmyk {
            budget.functions.charge_product(area(bounds), cost)?;
        }
        budget.bytes.charge(size_of::<Conversion>())?;
        let id = u32::try_from(self.parts.conversions.len())
            .map_err(|_| Error::limit("conversion count exceeds u32"))?;
        self.parts.conversions.push(Conversion {
            from,
            to,
            black,
            removal,
        });
        Ok(id)
    }

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
            .map_or(0, |f| f.cost());
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
        let key = (id, scope.transform_bits(), scope.layout.process);
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
        let format = self
            .storage
            .masks
            .get(&(id, scope.layout.process))
            .copied()
            .unwrap_or(Format::F64);
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
        };
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
        budget.bytes.charge(size_of::<MaskPlan>())?;
        let id = u32::try_from(self.parts.masks.len())
            .map_err(|_| Error::limit("soft mask count exceeds u32"))?;
        self.parts.masks.push(plan);
        self.users.push(DeviceRect::default());
        self.masks.insert(key, id);
        Ok(id)
    }

    /// The instances of a tiling pattern meeting `bounds`, as one
    /// non-isolated, non-knockout group composited through the painted
    /// mark's coverage (8.7.3, 11.6.7); every instance has its own exact
    /// geometry.
    pub(crate) fn pattern(
        &mut self,
        scope: &Scope,
        paint: &TilingPaint,
        bounds: DeviceRect,
        budget: &mut Budget,
    ) -> Result<Option<(u32, DeviceRect)>> {
        let scene = self.scene;
        let tiling = scene
            .tilings
            .get(index(paint.pattern.0))
            .ok_or(Error::invalid_argument("scene pattern out of range"))?;
        let cell = scene
            .group(tiling.cell)
            .ok_or(Error::invalid_argument("scene group out of range"))?;
        let transform = fit(tiling.matrix.then(scope.transform), tiling)?;
        let inverse = transform.inverse().ok_or(Error::unsupported(
            crate::features::PATTERN_TILING,
            "singular tiling transform",
        ))?;
        let (columns, rows) = instances(bounds, inverse, tiling)?;
        let count = (columns.1 - columns.0 + 1).saturating_mul(rows.1 - rows.0 + 1);
        if usize::try_from(count).map_or(true, |n| n > self.limits.display_commands) {
            return Err(Error::limit("tiling instance budget exceeded"));
        }
        let override_paint = match tiling.colors {
            TilingColors::Colored => None,
            TilingColors::Uncolored => {
                let color = paint
                    .color
                    .clone()
                    .ok_or(Error::syntax("uncoloured pattern without a colour"))?;
                let state = Arc::new(pagura_interpret::Composite::default());
                let paint = Arc::new(Paint::Solid(color));
                let index = self.solid_paint(&paint, (&state, scope), bounds, budget)?;
                Some((index, paint))
            }
        };
        let transparent = self
            .storage
            .cells
            .get(&(paint.pattern, scope.layout.process))
            .copied()
            .unwrap_or(false);
        let group = self.new_group(
            GroupPlan {
                bounds,
                items: Vec::new(),
                order: Vec::new(),
                layout: scope.layout,
                format: Format::F64,
                knockout: false,
                start: if transparent {
                    Start::Transparent
                } else {
                    Start::Backdrop
                },
            },
            budget,
        )?;
        let [x_step, y_step] = tiling.step;
        for j in rows.0..=rows.1 {
            for i in columns.0..=columns.1 {
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "instance indices are below 2^53"
                )]
                let offset = Transform::translate(i as f64 * x_step, j as f64 * y_step);
                let inner = Scope {
                    group,
                    transform: offset.then(transform),
                    crop: false,
                    bounds,
                    layout: scope.layout,
                    plain: false,
                    records: false,
                    separated: scope.separated,
                    masked: scope.masked,
                    paint: override_paint.clone().or_else(|| scope.paint.clone()),
                    depth: scope.depth + 1,
                };
                self.children(&inner, &cell.children, budget)?;
            }
        }
        let content = intersect(self.content(group), bounds);
        if let Some(plan) = self.parts.groups.get_mut(index(group)) {
            plan.bounds = content;
        }
        Ok((!content.is_empty()).then_some((group, content)))
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
                let f = curve(f)?;
                [Some(Arc::clone(&f)), Some(Arc::clone(&f)), Some(f)]
            }
            pagura_interpret::Transfer::PerComponent(functions) => {
                let mut out: [Option<Arc<dyn Curve>>; 3] = [None, None, None];
                for (slot, f) in out.iter_mut().zip(functions) {
                    *slot = f.as_ref().map(|f| curve(f)).transpose()?;
                }
                out
            }
        };
        budget
            .bytes
            .charge(size_of::<[Option<Arc<dyn Curve>>; 3]>())?;
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

/// A planned group whose result is composited into its parent.
struct GroupResult<'a> {
    group: u32,
    state: &'a pagura_interpret::Composite,
    culling: DeviceRect,
    serial: u32,
    position: Position,
    mask: Option<u32>,
}

impl Scope {
    /// The bits of the transform, as a key.
    pub(crate) fn transform_bits(&self) -> [u64; 6] {
        let t = self.transform;
        [t.a, t.b, t.c, t.d, t.e, t.f].map(f64::to_bits)
    }
}

/// A soft mask's `BC` and `TR`.
type MaskParameters = ([f64; 4], Option<Arc<dyn Curve>>);

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
    let transfer = mask.transfer.as_ref().map(curve).transpose()?;
    Ok((backdrop, transfer))
}

/// A one-input function as a curve; the first output is used.
pub(crate) fn curve(function: &Arc<pagura_color::Function>) -> Result<Arc<dyn Curve>> {
    if function.inputs() != 1 || function.outputs() == 0 {
        return Err(Error::syntax("transfer function must map one input"));
    }
    Ok(Arc::clone(function) as Arc<dyn Curve>)
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

/// The instance index ranges of a tiling pattern whose cells can meet
/// `bounds`: cells of `BBox` at multiples of `XStep` and `YStep`.
fn instances(
    bounds: DeviceRect,
    inverse: Transform,
    tiling: &pagura_interpret::Tiling,
) -> Result<((i64, i64), (i64, i64))> {
    let corners = [
        (bounds.x0, bounds.y0),
        (bounds.x1, bounds.y0),
        (bounds.x0, bounds.y1),
        (bounds.x1, bounds.y1),
    ]
    .map(|(x, y)| inverse.apply(pagura_raster::Point::new(f64::from(x), f64::from(y))));
    let mut hull = bounds::points(corners).unwrap_or([0.0; 4]);
    let b = tiling.bbox;
    if b.is_empty() {
        // Cells whose one-pixel boundary reaches the bounds.
        let px = inverse.a.abs() + inverse.c.abs();
        let py = inverse.b.abs() + inverse.d.abs();
        hull = [hull[0] - px, hull[1] - py, hull[2] + px, hull[3] + py];
    }
    let columns = range(hull[0], hull[2], (b.x0(), b.x1()), tiling.step[0])?;
    let rows = range(hull[1], hull[3], (b.y0(), b.y1()), tiling.step[1])?;
    Ok((columns, rows))
}

/// The device transform of a tiling pattern's cells (Table 74): constant
/// spacing rounds the lattice vectors `XStep·(a, b)` and `YStep·(c, d)` to
/// whole pixels, a vanishing one to a unit step along its larger component;
/// `TilingType 1` refuses a cell corner moved by more than a pixel, and
/// `TilingType 2` keeps the exact transform.
fn fit(t: Transform, tiling: &pagura_interpret::Tiling) -> Result<Transform> {
    if tiling.spacing == TilingSpacing::NoDistortion {
        return Ok(t);
    }
    let vector = |x: f64, y: f64| {
        let (mut a, mut b) = (x.round(), y.round());
        if a == 0.0 && b == 0.0 {
            if x.abs() >= y.abs() {
                a = x.signum();
            } else {
                b = y.signum();
            }
        }
        (a, b)
    };
    let [x_step, y_step] = tiling.step;
    let (a, b) = vector(t.a * x_step, t.b * x_step);
    let (c, d) = vector(t.c * y_step, t.d * y_step);
    let fitted = Transform::new(a / x_step, b / x_step, c / y_step, d / y_step, t.e, t.f);
    if fitted.inverse().is_none() {
        return Err(Error::unsupported(
            crate::features::PATTERN_TILING,
            "device lattice collapses the pattern",
        ));
    }
    if tiling.spacing == TilingSpacing::Constant {
        let b = tiling.bbox;
        for (x, y) in [
            (b.x0(), b.y0()),
            (b.x1(), b.y0()),
            (b.x0(), b.y1()),
            (b.x1(), b.y1()),
        ] {
            let p = pagura_raster::Point::new(x, y);
            let (exact, moved) = (t.apply(p), fitted.apply(p));
            if (exact.x - moved.x).hypot(exact.y - moved.y) > 1.0 + 1e-12 {
                return Err(Error::unsupported(
                    crate::features::PATTERN_TILING,
                    "constant spacing requires excessive cell distortion",
                ));
            }
        }
    }
    Ok(fitted)
}

/// The integers `k` for which `[b0, b1] + k·step` meets `[lo, hi]`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "integral values range-checked within exact f64 integers"
)]
fn range(lo: f64, hi: f64, (b0, b1): (f64, f64), step: f64) -> Result<(i64, i64)> {
    let a = (lo - b1) / step;
    let b = (hi - b0) / step;
    let start = a.min(b).ceil();
    let end = a.max(b).floor();
    if !start.is_finite() || !end.is_finite() || start.abs().max(end.abs()) > 4e15 {
        return Err(Error::limit("tiling index exceeds numeric range"));
    }
    Ok((start as i64, (end as i64).max(start as i64 - 1)))
}

/// The page crop as a path, in default user space.
pub(crate) fn crop(scene: &Scene) -> Path {
    Path::rect(scene.page.crop)
}

/// The tops of the stacks needed to run `group`, from the innermost buffer
/// out.
pub(crate) fn needs(parts: &Parts, rows: usize) -> Tops {
    let mut memo: Vec<Option<Tops>> = vec![None; parts.groups.len()];
    need(parts, 0, rows, &mut memo, 0)
}

fn need(
    parts: &Parts,
    group: u32,
    rows: usize,
    memo: &mut Vec<Option<Tops>>,
    depth: usize,
) -> Tops {
    if let Some(Some(done)) = memo.get(index(group)) {
        return *done;
    }
    let Some(plan) = parts.groups.get(index(group)) else {
        return Tops::default();
    };
    if depth > parts.groups.len() {
        return Tops::default();
    }
    let mut most = Tops::default();
    let mask_need = |mask: Option<u32>, memo: &mut Vec<Option<Tops>>| {
        mask.and_then(|m| parts.masks.get(index(m)))
            .map_or(Tops::default(), |m| {
                need(parts, m.group, rows, memo, depth + 1)
            })
    };
    for &id in &plan.items {
        let Some(item) = parts.items.get(index(id)) else {
            continue;
        };
        let inner = match &item.kind {
            ItemKind::Mark(mark) => mask_need(mark.compositing.mask, memo),
            ItemKind::Run(run) => mask_need(run.mark.compositing.mask, memo),
            ItemKind::Group(used) => {
                let child = need(parts, used.group, rows, memo, depth + 1);
                let own = own(parts, used.group, rows);
                max(child, add(own, mask_need(used.compositing.mask, memo)))
            }
        };
        most = max(most, inner);
    }
    let total = if group == 0 {
        most
    } else {
        add(own(parts, group, rows), most)
    };
    if let Some(slot) = memo.get_mut(index(group)) {
        *slot = Some(total);
    }
    total
}

fn own(parts: &Parts, group: u32, rows: usize) -> Tops {
    let Some(plan) = parts.groups.get(index(group)) else {
        return Tops::default();
    };
    let pixels = pagura_core::convert::u32_to_usize(plan.bounds.width())
        .saturating_mul(pagura_core::convert::u32_to_usize(plan.bounds.height()).min(rows));
    match plan.format {
        Format::F64 => Tops {
            wide: pixels.saturating_mul(plan.layout.stride()),
            ..Tops::default()
        },
        Format::U8 => Tops {
            bytes: pixels,
            ..Tops::default()
        },
        Format::F32 | Format::N16 => Tops {
            levels: pixels,
            ..Tops::default()
        },
    }
}

fn add(a: Tops, b: Tops) -> Tops {
    Tops {
        wide: a.wide.saturating_add(b.wide),
        levels: a.levels.saturating_add(b.levels),
        bytes: a.bytes.saturating_add(b.bytes),
    }
}

fn max(a: Tops, b: Tops) -> Tops {
    Tops {
        wide: a.wide.max(b.wide),
        levels: a.levels.max(b.levels),
        bytes: a.bytes.max(b.bytes),
    }
}
