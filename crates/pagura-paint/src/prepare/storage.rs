//! Storage formats from whole content (ADR 0004 §5–§6), decided before any
//! region is considered: one pass over the page's content describes every
//! buffer (the page, each transparency group, soft mask and tiling cell) by
//! its stores, then formats are chosen from the innermost buffer outwards
//! and widened from the outermost inwards. The same pass decides which
//! groups composite as their members (flattening), which paint nothing,
//! which run from a transparent start, and whether the page selects
//! transfer functions.

mod formats;
mod hulls;

use formats::{F64_STORE, FOOTPRINTS, MARGIN};
use hulls::{Outlines, extent, hulls, unite};

use crate::format::Format;
use crate::plan::Layout;
use crate::prepare::bounds;
use crate::prepare::index;
use crate::prepare::overprint::{Keeping, Place};
use crate::prepare::planner::view_transform;
use pagura_color::Process;
use pagura_core::hash::FastMap;
use pagura_core::{Error, Meter, Result};
use pagura_interpret::{
    BlendMode, Composite, Group, GroupAttributes, GroupId, GroupRole, ImageMask, MaskId, Node,
    Paint, PatternId, Scene,
};
use pagura_raster::{DeviceRect, Transform};

/// How a transparency group reaches its parent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Route {
    /// Through its own buffer.
    Buffer,
    /// Its members composite into the parent directly.
    Flatten,
    /// Its one member composites into the parent directly, at the group's
    /// alpha times its own.
    Fold,
    /// Not at all: its result's alpha is 0 everywhere, which leaves the
    /// parent's colour and alpha as they are, and no buffer it reaches
    /// knocks out by its shape.
    Skip,
}

/// What preparation needs to know about one transparency group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Decision {
    pub(crate) route: Route,
    /// Its buffer's format.
    pub(crate) format: Format,
    /// It starts transparent: isolated, or non-isolated with Normal content,
    /// which composites as an isolated group (11.4.4).
    pub(crate) transparent: bool,
    /// Its members knock each other out; false for the opaque-member
    /// identity, which composites as source-over (11.4.6).
    pub(crate) knockout: bool,
}

/// The decisions for a page.
#[derive(Debug)]
pub(crate) struct Storage {
    /// The page buffer's format.
    pub(crate) page: Format,
    /// Whether the page's knockout is real (not the opaque-member identity).
    pub(crate) page_knockout: bool,
    /// Transparency groups by group and parent blending space.
    pub(crate) groups: FastMap<(GroupId, Process), Decision>,
    /// Soft masks by mask and parent blending space.
    pub(crate) masks: FastMap<(MaskId, Process), Format>,
    /// Whether the instances of a tiling pattern start transparent (Normal
    /// cell content), by pattern and blending space.
    pub(crate) cells: FastMap<(PatternId, Process), bool>,
    /// Some page content selects a transfer function (11.7.5.2).
    pub(crate) transfers: bool,
    /// Some content outside soft masks, tiling cells included, has a
    /// transfer function: pattern-painted and simulated overprinting marks
    /// outside soft masks are then unsupported.
    pub(crate) transfer_gate: bool,
}

impl Storage {
    /// The decision tables retained by the planner after survey scratch drops.
    pub(crate) fn retained_bytes(&self, bytes: &Meter) -> Result<usize> {
        let groups = bytes.map_bytes(&self.groups)?;
        let masks = bytes.map_bytes(&self.masks)?;
        let cells = bytes.map_bytes(&self.cells)?;
        groups
            .checked_add(masks)
            .and_then(|total| total.checked_add(cells))
            .ok_or(Error::limit("render memory budget exceeded"))
    }
}

/// The inputs of a survey.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Inputs {
    /// The page's channels.
    pub(crate) page: Layout,
    /// Overprint simulation with the page's separations.
    pub(crate) simulate: bool,
    /// The overprint simulation policy, page separations or not.
    pub(crate) policy: bool,
    /// The nesting budget.
    pub(crate) depth: usize,
    /// Default user space to device pixels.
    pub(crate) transform: Transform,
    /// The page's device pixels.
    pub(crate) pixels: DeviceRect,
    /// `UserUnit`, for fixed views.
    pub(crate) user_unit: f64,
}

/// Surveys `scene`.
///
/// # Errors
///
/// A `Limit` error for nesting beyond `inputs.depth` or exhausted render memory,
/// `Syntax` for a soft
/// mask whose content uses itself and `InvalidArgument` for ids out of
/// range.
pub(crate) fn survey(scene: &Scene, inputs: Inputs, bytes: &mut Meter) -> Result<Storage> {
    let before = bytes.used();
    let survey = Survey {
        scene,
        inputs,
        // A mark records the transfer selection of the pixels it covers
        // whatever its alpha (11.7.5.2).
        skips: scene.states.iter().all(|state| state.transfer.is_none()),
        buffers: Vec::new(),
        groups: FastMap::default(),
        masks: FastMap::default(),
        cells: FastMap::default(),
        open: Vec::new(),
        outlines: Outlines::default(),
        transfers: false,
    };
    let result = survey.finish(bytes);
    let retained = match &result {
        Ok(storage) => storage.retained_bytes(bytes)?,
        Err(_) => 0,
    };
    // finish consumed the survey; only the returned decision maps remain live.
    bytes.refund(bytes.used() - before - retained);
    result
}

impl Survey<'_> {
    fn finish(mut self, bytes: &mut Meter) -> Result<Storage> {
        let inputs = self.inputs;
        let scene = self.scene;
        let root = scene
            .group(scene.root)
            .ok_or(Error::invalid_argument("scene group out of range"))?;
        let page = self.buffer(inputs.page, root.attributes.knockout, bytes)?;
        self.buffers[page].page = true;
        self.children(
            page,
            &root.children,
            Context::page(inputs.transform),
            0,
            bytes,
        )?;
        let page_knockout = self.buffers[page].knockout();
        let transfers = self.transfers;
        let transfer_gate = self.buffers[page].transfer;
        let buffer = &mut self.buffers[page];
        buffer.forced |=
            inputs.simulate || inputs.page.process != Process::Rgb || page_knockout || transfers;
        let mut resolved = Vec::new();
        bytes.reserve_capacity(&mut resolved, self.buffers.len())?;
        resolved.resize(self.buffers.len(), None);
        self.resolve(page, &mut resolved, 0, bytes)?;
        self.widen(page, &mut resolved, bytes)?;
        let format = |node: usize| {
            resolved
                .get(node)
                .copied()
                .flatten()
                .map_or(Format::F64, |(format, _)| format)
        };
        let mut groups = FastMap::default();
        bytes.reserve_map_capacity(&mut groups, self.groups.len())?;
        for (&key, entry) in &self.groups {
            let decision = Decision {
                route: entry.route,
                format: format(entry.buffer),
                transparent: entry.transparent,
                knockout: entry.knockout,
            };
            groups.insert(key, decision);
        }
        let mut masks = FastMap::default();
        bytes.reserve_map_capacity(&mut masks, self.masks.len())?;
        for (&key, &node) in &self.masks {
            masks.insert(key, format(node));
        }
        let mut cells = FastMap::default();
        bytes.reserve_map_capacity(&mut cells, self.cells.len())?;
        for (&key, &node) in &self.cells {
            cells.insert(key, self.buffers[node].normal_content());
        }
        Ok(Storage {
            page: format(page),
            page_knockout,
            groups,
            masks,
            cells,
            transfers,
            transfer_gate,
        })
    }
}

/// Where content is.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Context {
    /// Page content, whose marks select transfer functions.
    selects: bool,
    /// Soft-mask content.
    masked: bool,
    /// A buffer enclosing the parent knocks out, or the content is a tiling
    /// cell's, whose shape reaches wherever the pattern paints.
    knockout: bool,
    /// The alpha of a folded group, which its member's multiplies; 1
    /// elsewhere.
    alpha: f64,
    /// The soft mask of a folded group, a buffer, which its member
    /// composites through when it has none of its own.
    mask: Option<usize>,
    /// Content space to device pixels.
    transform: Transform,
}

impl Context {
    fn page(transform: Transform) -> Self {
        Self {
            selects: true,
            masked: false,
            knockout: false,
            alpha: 1.0,
            mask: None,
            transform,
        }
    }

    /// A soft mask's values are its group's colour and alpha, never shape.
    fn mask(self) -> Self {
        Self {
            selects: false,
            masked: true,
            knockout: false,
            alpha: 1.0,
            mask: None,
            ..self
        }
    }
}

/// A dynamic store of a buffer (ADR 0004 §6): the pixels it can reach and
/// what it injects.
#[derive(Debug, Clone, Copy)]
struct Dynamic {
    /// Device pixels, conservatively.
    rect: DeviceRect,
    /// Its own error, in levels.
    eta: f64,
    /// The child whose result it composites.
    child: Option<usize>,
    /// The soft mask of its opacity.
    mask: Option<usize>,
}

/// A buffer described by its stores.
#[derive(Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent facts of a buffer's content"
)]
struct Buffer {
    layout: Layout,
    page: bool,
    /// Every format but F64 is ruled out.
    forced: bool,
    /// `K` of the group attributes.
    knockout_attribute: bool,
    /// Every store's opacity equals its shape.
    opaque_members: bool,
    /// Every store's opacity after the first equals its shape.
    later_opaque: bool,
    /// Every store blends Normal.
    normal: bool,
    /// Some store's alpha can be positive.
    visible: bool,
    /// It provides the backdrop of a non-isolated child whose content is not
    /// all Normal.
    reads: bool,
    /// Stores at a pixel, at most.
    stores: usize,
    /// Dynamic stores among them.
    dynamic: usize,
    /// The least positive opacity of the contracting stores.
    theta: f64,
    /// Opaque constant stores only, apart from the dynamic ones.
    eta: f64,
    /// Child buffers composited into it, with their counts.
    children: Vec<(usize, usize)>,
    /// Masks of its stores, with their counts.
    masks: Vec<(usize, usize)>,
    /// Its dynamic stores, by footprint.
    dynamics: Vec<Dynamic>,
    /// The device pixels its stores can reach.
    extent: Option<bounds::Hull>,
    /// Plain groups of opaque members that flatten into it if its knockout
    /// turns out to be the opaque-member identity, with their buffers.
    pending: Vec<((GroupId, Process), usize)>,
    /// Some mark of its content, groups and tiling cells included and soft
    /// masks not, has a transfer function.
    transfer: bool,
}

impl Buffer {
    /// Its members knock each other out: unless every store after the
    /// first has opacity equal to its shape, where knockout is source-over
    /// (11.4.6). The first store meets the initial backdrop, where it is
    /// source-over at any opacity.
    fn knockout(&self) -> bool {
        self.knockout_attribute && !self.later_opaque
    }

    /// Normal content: compositing it onto a backdrop equals compositing
    /// it alone and then onto the backdrop.
    fn normal_content(&self) -> bool {
        self.normal && !self.knockout() && !self.reads
    }

    fn merge(&mut self, other: &Self, bytes: &mut Meter) -> Result<()> {
        self.forced |= other.forced;
        self.opaque_members &= other.opaque_members;
        // Merged stores follow its own.
        self.later_opaque &= other.opaque_members;
        self.normal &= other.normal;
        self.visible |= other.visible;
        self.reads |= other.reads;
        self.stores = self.stores.saturating_add(other.stores);
        self.dynamic = self.dynamic.saturating_add(other.dynamic);
        self.theta = self.theta.min(other.theta);
        self.eta += other.eta;
        bytes.reserve_capacity(&mut self.children, other.children.len())?;
        self.children.extend_from_slice(&other.children);
        bytes.reserve_capacity(&mut self.masks, other.masks.len())?;
        self.masks.extend_from_slice(&other.masks);
        bytes.reserve_capacity(&mut self.dynamics, other.dynamics.len())?;
        self.dynamics.extend_from_slice(&other.dynamics);
        self.extent = unite(self.extent, other.extent);
        self.transfer |= other.transfer;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
struct GroupEntry {
    buffer: usize,
    route: Route,
    transparent: bool,
    knockout: bool,
}

struct Survey<'s> {
    scene: &'s Scene,
    inputs: Inputs,
    /// Groups that paint nothing can be left out.
    skips: bool,
    buffers: Vec<Buffer>,
    groups: FastMap<(GroupId, Process), GroupEntry>,
    masks: FastMap<(MaskId, Process), usize>,
    cells: FastMap<(PatternId, Process), usize>,
    open: Vec<(MaskId, Process)>,
    /// Glyph outline hulls computed so far.
    outlines: Outlines,
    transfers: bool,
}

/// The kind of paint a store takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Constant,
    Varying,
    Pattern(PatternId),
}

impl Survey<'_> {
    fn buffer(&mut self, layout: Layout, knockout: bool, bytes: &mut Meter) -> Result<usize> {
        bytes.reserve_capacity(&mut self.buffers, 1)?;
        self.buffers.push(Buffer {
            layout,
            page: false,
            forced: self.inputs.simulate || layout.process != Process::Rgb,
            knockout_attribute: knockout,
            opaque_members: true,
            later_opaque: true,
            normal: true,
            visible: false,
            reads: false,
            stores: 0,
            dynamic: 0,
            theta: 1.0,
            eta: 0.0,
            children: Vec::new(),
            masks: Vec::new(),
            dynamics: Vec::new(),
            extent: None,
            pending: Vec::new(),
            transfer: false,
        });
        Ok(self.buffers.len() - 1)
    }

    fn children(
        &mut self,
        buffer: usize,
        children: &[pagura_interpret::NodeId],
        context: Context,
        depth: usize,
        bytes: &mut Meter,
    ) -> Result<()> {
        if depth >= self.inputs.depth {
            return Err(Error::limit("compositing depth exceeded"));
        }
        let scene = self.scene;
        for &id in children {
            let node = scene
                .node(id)
                .ok_or(Error::invalid_argument("scene node out of range"))?;
            let (mark, kind, count, alpha_shape) = match node {
                Node::Fill(fill) => (&fill.mark, self.paint(fill.paint)?, 1, true),
                Node::Stroke(stroke) => (&stroke.mark, self.paint(stroke.paint)?, 1, true),
                Node::Glyphs(glyphs) => (
                    &glyphs.mark,
                    self.paint(glyphs.paint)?,
                    glyphs.placements.len(),
                    true,
                ),
                // A soft mask or embedded opacity gives an image alpha that
                // is not its shape unless AIS.
                Node::Image(image) => (
                    &image.mark,
                    Kind::Varying,
                    1,
                    !matches!(
                        image.image.mask,
                        ImageMask::Soft { .. } | ImageMask::Embedded
                    ),
                ),
                Node::Shading(shading) => (&shading.mark, Kind::Varying, 1, true),
                Node::Group(group) => {
                    self.group(buffer, *group, context, depth, bytes)?;
                    continue;
                }
            };
            let state = folded(state(scene, mark.state)?, context);
            let state = &*state;
            // Footprints matter for dynamic stores, and the extent of a
            // group, whose result is a dynamic store of its parent.
            let (hulls, extent) = if dynamic(kind, state) || context.mask.is_some() {
                let hulls = hulls(scene, node, context.transform, (&mut self.outlines, bytes))?;
                let extent = hulls.iter().fold(None, |extent, &hull| unite(extent, hull));
                (hulls, extent)
            } else if self.buffers[buffer].page {
                (Vec::new(), None)
            } else {
                let extent = extent(scene, node, context.transform, (&mut self.outlines, bytes))?;
                (Vec::new(), extent)
            };
            let stores = Stores {
                kind,
                count,
                hulls: &hulls,
                extent,
                alpha_shape,
            };
            self.mark(buffer, state, &stores, context, depth, bytes)?;
            let released = hulls.capacity() * size_of::<Option<bounds::Hull>>();
            drop(hulls);
            bytes.refund(released);
        }
        self.settle(buffer, bytes)
    }

    /// Flattens the pending groups of `buffer` once its content shows that
    /// its knockout is the opaque-member identity, so that it composites as
    /// source-over and they can composite straight into it.
    fn settle(&mut self, buffer: usize, bytes: &mut Meter) -> Result<()> {
        let pending = std::mem::take(&mut self.buffers[buffer].pending);
        if !self.buffers[buffer].knockout() {
            for &(key, child) in &pending {
                let target = &mut self.buffers[buffer];
                // Its result was one dynamic store.
                target.stores = target.stores.saturating_sub(1);
                target.dynamic = target.dynamic.saturating_sub(1);
                target.dynamics.retain(|store| store.child != Some(child));
                target.children.retain(|&(c, _)| c != child);
                self.merge(buffer, child, bytes)?;
                if let Some(entry) = self.groups.get_mut(&key) {
                    entry.route = Route::Flatten;
                }
            }
        }
        let released = pending.capacity() * size_of::<((GroupId, Process), usize)>();
        drop(pending);
        bytes.refund(released);
        Ok(())
    }

    fn merge(&mut self, parent: usize, child: usize, bytes: &mut Meter) -> Result<()> {
        debug_assert!(parent < child);
        let (parents, children) = self.buffers.split_at_mut(child);
        parents[parent].merge(&children[0], bytes)
    }

    /// The device pixels of `hull` on the page.
    fn pixels(&self, hull: Option<bounds::Hull>) -> DeviceRect {
        bounds::device(hull, self.inputs.pixels)
    }

    fn paint(&self, id: pagura_interpret::PaintId) -> Result<Kind> {
        let paint = self
            .scene
            .paints
            .get(id.index())
            .ok_or(Error::invalid_argument("scene paint out of range"))?;
        Ok(match &**paint {
            Paint::Solid(_) => Kind::Constant,
            Paint::Shading(_) => Kind::Varying,
            Paint::Tiling(tiling) => Kind::Pattern(tiling.pattern),
        })
    }

    fn mark(
        &mut self,
        buffer: usize,
        state: &Composite,
        stores: &Stores<'_>,
        context: Context,
        depth: usize,
        bytes: &mut Meter,
    ) -> Result<()> {
        let (kind, count) = (stores.kind, stores.count);
        let layout = self.buffers[buffer].layout;
        let mask = self.soft_mask(state, layout.process, (context, depth), bytes)?;
        let cell = match kind {
            Kind::Pattern(pattern) => Some(self.cell(pattern, layout, (context, depth), bytes)?),
            Kind::Constant | Kind::Varying => None,
        };
        let mut rects = Vec::new();
        bytes.reserve_capacity(&mut rects, stores.hulls.len())?;
        rects.extend(stores.hulls.iter().map(|&hull| self.pixels(hull)));
        if state.transfer.is_some() && context.selects {
            self.transfers = true;
        }
        let cell_reads = cell.is_some_and(|c| !self.buffers[c].normal_content());
        let transfer = state.transfer.is_some() || cell.is_some_and(|c| self.buffers[c].transfer);
        // Conservatively, as any colour of an uncoloured cell may paint it.
        let keeps = Keeping {
            separated: self.inputs.simulate,
            policy: self.inputs.policy,
        }
        .keeps(
            state,
            Place {
                masked: context.masked,
                cmyk: layout.process == Process::Cmyk,
                direct_cmyk: true,
            },
        );
        let target = &mut self.buffers[buffer];
        let first = target.stores == 0 && count == 1;
        target.stores = target.stores.saturating_add(count);
        target.transfer |= transfer;
        target.visible |= state.alpha != 0.0;
        // A store that keeps channels reads its backdrop.
        target.normal &= state.blend == BlendMode::Normal && !keeps;
        // A store with a transfer function is outside the narrow bounds
        // wherever it is (ADR 0004 §5).
        target.forced |= state.blend != BlendMode::Normal || state.transfer.is_some();
        target.extent = unite(target.extent, stores.extent);
        // Opacity equals shape, and a knockout of such stores is source-over
        // only when they blend Normal and keep no channel of the backdrop.
        let shaped =
            stores.alpha_shape && (state.alpha_is_shape || (state.alpha == 1.0 && mask.is_none()));
        let member = cell.is_none() && state.blend == BlendMode::Normal && !keeps;
        target.opaque_members &= shaped && member;
        target.later_opaque &= (shaped || first) && member;
        if cell_reads {
            // A cell whose content reads its backdrop reads this buffer.
            target.reads = true;
            target.forced = true;
        }
        if let Some(mask) = mask {
            bytes.reserve_capacity(&mut target.masks, 1)?;
            target.masks.push((mask, count));
        }
        if let Some(cell) = cell {
            bytes.reserve_capacity(&mut target.children, 1)?;
            target.children.push((cell, count));
        }
        let constant = kind == Kind::Constant;
        if constant && mask.is_none() && state.alpha > 0.0 {
            target.theta = target
                .theta
                .min(state.alpha / 255.0 * (1.0 - 1.0 / 4_194_304.0));
        }
        if dynamic(kind, state) {
            // A varying source's f64 evaluation is within the margin; a
            // translucent constant's store errs by one f64 evaluation.
            let eta = if kind == Kind::Varying {
                MARGIN
            } else {
                F64_STORE
            };
            target.dynamic = target.dynamic.saturating_add(count);
            target.eta += eta * count as f64;
            if target.dynamics.len() <= FOOTPRINTS {
                bytes.reserve_capacity(&mut target.dynamics, rects.len())?;
                target.dynamics.extend(rects.iter().map(|&rect| Dynamic {
                    rect,
                    eta,
                    child: cell,
                    mask,
                }));
            }
        }
        let released = rects.capacity() * size_of::<DeviceRect>();
        drop(rects);
        bytes.refund(released);
        Ok(())
    }

    fn group(
        &mut self,
        parent: usize,
        id: GroupId,
        context: Context,
        depth: usize,
        bytes: &mut Meter,
    ) -> Result<()> {
        let scene = self.scene;
        let group = scene
            .group(id)
            .ok_or(Error::invalid_argument("scene group out of range"))?;
        if group.role != GroupRole::Transparency {
            return Ok(());
        }
        let state = folded(state(scene, group.mark.state)?, context);
        let state = &*state;
        let parent_layout = self.buffers[parent].layout;
        let layout = group_layout(group.attributes, parent_layout);
        let child = self.buffer(layout, group.attributes.knockout, bytes)?;
        let parent_knockout = self.buffers[parent].knockout_attribute;
        let transform = group.view.map_or(Ok(context.transform), |view| {
            view_transform(context.transform, view, self.inputs.user_unit)
        })?;
        let fold = self.folds((group, state), (parent, layout), context)?;
        // A folded group's soft mask, its own or the one it took, passes to
        // its member.
        let mask = if fold {
            self.soft_mask(state, parent_layout.process, (context, depth), bytes)?
        } else {
            None
        };
        let inner = Context {
            transform,
            knockout: context.knockout || parent_knockout,
            alpha: if fold { state.alpha } else { 1.0 },
            mask,
            ..context
        };
        self.children(child, &group.children, inner, depth + 1, bytes)?;
        let facts = &self.buffers[child];
        let fits = plain(state, context)
            && !facts.knockout()
            && layout == parent_layout
            && (!group.attributes.isolated || facts.normal_content());
        // A result of alpha 0 changes the parent's shape alone, which only
        // a knockout reads (11.4.6), above it or in a pattern's instances.
        let route = if self.skips && !inner.knockout && (!facts.visible || state.alpha == 0.0) {
            Route::Skip
        } else if fold {
            Route::Fold
        } else if fits && !parent_knockout {
            Route::Flatten
        } else {
            Route::Buffer
        };
        // In a knockout group, a plain group of opaque members is one opaque
        // member: the implicit groups of text objects and of fill-and-stroke
        // operators around Type 3 glyphs or fill-and-stroke text.
        let pending = fits && parent_knockout && facts.opaque_members;
        let transparent = group.attributes.isolated || facts.normal_content();
        let knockout = facts.knockout();
        let extent = facts.extent;
        let transfer = facts.transfer;
        let visible = facts.visible && state.alpha != 0.0;
        let entry = GroupEntry {
            buffer: child,
            route,
            transparent,
            knockout,
        };
        self.record((id, parent_layout.process), entry, bytes)?;
        match route {
            Route::Skip => return Ok(()),
            Route::Flatten | Route::Fold => return self.merge(parent, child, bytes),
            Route::Buffer => {}
        }
        let mask = self.soft_mask(state, parent_layout.process, (context, depth), bytes)?;
        let buffer = &mut self.buffers[child];
        buffer.forced |= state.blend.amplifies() || knockout || !transparent;
        let rect = self.pixels(extent);
        let target = &mut self.buffers[parent];
        target.extent = unite(target.extent, extent);
        bytes.reserve_capacity(&mut target.dynamics, 1)?;
        target.dynamics.push(Dynamic {
            rect,
            eta: 0.0,
            child: Some(child),
            mask,
        });
        target.stores = target.stores.saturating_add(1);
        target.dynamic = target.dynamic.saturating_add(1);
        target.normal &= state.blend == BlendMode::Normal;
        target.forced |= state.blend != BlendMode::Normal;
        target.transfer |= transfer;
        target.visible |= visible;
        target.opaque_members &= pending;
        target.later_opaque &= pending;
        if pending {
            bytes.reserve_capacity(&mut target.pending, 1)?;
            target.pending.push(((id, parent_layout.process), child));
        }
        bytes.reserve_capacity(&mut target.children, 1)?;
        target.children.push((child, 1));
        if let Some(mask) = mask {
            bytes.reserve_capacity(&mut target.masks, 1)?;
            target.masks.push((mask, 1));
        }
        if !transparent {
            target.reads = true;
            target.forced = true;
        }
        Ok(())
    }

    /// The soft mask a group or store composites through: its own, or that
    /// of the folded group it is the member of.
    fn soft_mask(
        &mut self,
        state: &Composite,
        process: Process,
        (context, depth): (Context, usize),
        bytes: &mut Meter,
    ) -> Result<Option<usize>> {
        match state.soft_mask {
            Some(id) => Ok(Some(self.mask(id, process, (context, depth), bytes)?)),
            None => Ok(context.mask),
        }
    }

    /// Records the facts of group `key`'s decision.
    fn record(
        &mut self,
        key: (GroupId, Process),
        entry: GroupEntry,
        bytes: &mut Meter,
    ) -> Result<()> {
        if !self.groups.contains_key(&key) {
            bytes.reserve_map_capacity(&mut self.groups, 1)?;
        }
        self.groups.insert(key, entry);
        Ok(())
    }

    /// Whether a group paints as its one member does at the group's alpha
    /// times its own and through the group's soft mask: a single store or
    /// group of Normal blend, whose source-over at opacity m a is the
    /// group's result composited at m (11.4.8), with no knockout, transfer
    /// or conversion between them and one soft mask at most. Alpha as shape
    /// changes shape alone, which only a knockout reads, above or in a
    /// pattern's instances.
    fn folds(
        &self,
        (group, state): (&Group, &Composite),
        (parent, layout): (usize, Layout),
        context: Context,
    ) -> Result<bool> {
        let target = &self.buffers[parent];
        let masked = state.soft_mask.is_some() || context.mask.is_some();
        if (state.alpha == 1.0 && !masked)
            || state.blend != BlendMode::Normal
            || (state.alpha_is_shape && context.knockout)
            || state.transfer.is_some()
            || group.attributes.knockout
            || target.knockout_attribute
            || layout != target.layout
        {
            return Ok(false);
        }
        let [child] = group.children[..] else {
            return Ok(false);
        };
        let (mark, store) = match self.scene.node(child) {
            Some(Node::Fill(fill)) => (&fill.mark, true),
            Some(Node::Stroke(stroke)) => (&stroke.mark, true),
            Some(Node::Image(image)) => (&image.mark, true),
            Some(Node::Shading(shading)) => (&shading.mark, true),
            Some(Node::Group(id)) => match self.scene.group(*id) {
                Some(member) if member.role == GroupRole::Transparency => (&member.mark, false),
                _ => return Ok(false),
            },
            _ => return Ok(false),
        };
        let member = self::state(self.scene, mark.state)?;
        let keeps = store
            && Keeping {
                separated: self.inputs.simulate,
                policy: self.inputs.policy,
            }
            .keeps(
                member,
                Place {
                    masked: context.masked,
                    cmyk: layout.process == Process::Cmyk,
                    direct_cmyk: true,
                },
            );
        Ok(member.blend == BlendMode::Normal
            && !(member.alpha_is_shape && context.knockout)
            && !(masked && member.soft_mask.is_some())
            && (store || member.transfer.is_none())
            && !keeps)
    }

    fn mask(
        &mut self,
        id: MaskId,
        process: Process,
        (context, depth): (Context, usize),
        bytes: &mut Meter,
    ) -> Result<usize> {
        let key = (id, process);
        if let Some(&buffer) = self.masks.get(&key) {
            return Ok(buffer);
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
        let layout = Layout {
            process: group.attributes.space.unwrap_or(process),
            spots: 0,
        };
        let buffer = self.buffer(layout, group.attributes.knockout, bytes)?;
        bytes.reserve_capacity(&mut self.open, 1)?;
        self.open.push(key);
        let walked = self.children(buffer, &group.children, context.mask(), depth + 1, bytes);
        self.open.pop();
        walked?;
        let facts = &mut self.buffers[buffer];
        let transparent = group.attributes.isolated || facts.normal_content();
        facts.forced |= mask.transfer.is_some() || facts.knockout() || !transparent;
        bytes.reserve_map_capacity(&mut self.masks, 1)?;
        self.masks.insert(key, buffer);
        Ok(buffer)
    }

    fn cell(
        &mut self,
        pattern: PatternId,
        layout: Layout,
        (context, depth): (Context, usize),
        bytes: &mut Meter,
    ) -> Result<usize> {
        if let Some(&buffer) = self.cells.get(&(pattern, layout.process)) {
            return Ok(buffer);
        }
        let scene = self.scene;
        let tiling = scene
            .tilings
            .get(index(pattern.0))
            .ok_or(Error::invalid_argument("scene pattern out of range"))?;
        let group = scene
            .group(tiling.cell)
            .ok_or(Error::invalid_argument("scene group out of range"))?;
        let buffer = self.buffer(layout, false, bytes)?;
        // Registered before its content: a cell painting itself is caught by
        // the nesting budget, as its instances nest without end.
        bytes.reserve_map_capacity(&mut self.cells, 1)?;
        self.cells.insert((pattern, layout.process), buffer);
        let inner = Context {
            selects: false,
            knockout: true,
            alpha: 1.0,
            transform: tiling.matrix.then(context.transform),
            ..context
        };
        self.children(buffer, &group.children, inner, depth + 1, bytes)?;
        self.buffers[buffer].forced = true;
        Ok(buffer)
    }
}

/// The blending space of a group: its own when isolated, else its parent's
/// (11.6.6); spot channels carry through.
pub(crate) fn group_layout(attributes: GroupAttributes, parent: Layout) -> Layout {
    if attributes.isolated {
        Layout {
            process: attributes.space.unwrap_or(parent.process),
            spots: parent.spots,
        }
    } else {
        parent
    }
}

fn state(scene: &Scene, id: pagura_interpret::StateId) -> Result<&Composite> {
    scene
        .states
        .get(id.index())
        .map(|state| &**state)
        .ok_or(Error::invalid_argument("scene state out of range"))
}

/// The stores of one node.
struct Stores<'h> {
    kind: Kind,
    /// Stores: one for most marks, one per placement for glyphs.
    count: usize,
    /// One device hull per store where footprints matter, else none.
    hulls: &'h [Option<bounds::Hull>],
    /// The device hull of all of them, where a group's extent needs it.
    extent: Option<bounds::Hull>,
    /// The source's alpha is its shape (not an image with a soft mask).
    alpha_shape: bool,
}

/// Whether a store of `kind` under `state` is dynamic (ADR 0004 §6): all
/// but opaque constant colours that only coverage reduces.
/// A group of alpha 1 and Normal blend through no soft mask, its own or a
/// folded group's: its result composites as its content.
fn plain(state: &Composite, context: Context) -> bool {
    state.alpha == 1.0
        && state.blend == BlendMode::Normal
        && state.soft_mask.is_none()
        && context.mask.is_none()
}

/// `state` of the member of a folded group: at the group's alpha times its
/// own.
fn folded(state: &Composite, context: Context) -> std::borrow::Cow<'_, Composite> {
    if context.alpha == 1.0 {
        std::borrow::Cow::Borrowed(state)
    } else {
        std::borrow::Cow::Owned(Composite {
            alpha: state.alpha * context.alpha,
            ..state.clone()
        })
    }
}

fn dynamic(kind: Kind, state: &Composite) -> bool {
    !(kind == Kind::Constant && state.soft_mask.is_none() && state.alpha == 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Build, mark};
    use pagura_core::ErrorKind;
    use pagura_interpret::{GroupAttributes, MaskKind, StateId};

    fn inputs(build: &Build) -> Inputs {
        Inputs {
            page: Layout {
                process: Process::Rgb,
                spots: 0,
            },
            simulate: false,
            policy: false,
            depth: 64,
            transform: build.transform(),
            pixels: build.page(),
            user_unit: build.scene.page.user_unit,
        }
    }

    #[test]
    fn survey_returns_only_owned_maps_and_refuses_temporary_overlap() {
        let mut build = Build::new(8.0, 6.0);
        let red = build.rgb([1.0, 0.0, 0.0]);
        let (mask, mask_group) = build.mask(MaskKind::Luminosity, GroupAttributes::default(), None);
        build.rect(mask_group, [0.0, 0.0, 6.0, 6.0], red, mark(StateId(0)));
        let masked = build.state(|state| {
            state.alpha = 0.5;
            state.soft_mask = Some(mask);
        });
        let group = build.transparency(GroupId(0), GroupAttributes::default(), mark(masked));
        build.rect(group, [0.0, 0.0, 7.0, 6.0], red, mark(StateId(0)));
        let mut bytes = Meter::new(1 << 20, "survey memory");
        bytes.charge(17).unwrap();
        let storage = survey(&build.scene, inputs(&build), &mut bytes).unwrap();
        assert_eq!((storage.groups.len(), storage.masks.len()), (1, 1));
        let retained = storage.retained_bytes(&bytes).unwrap();
        assert_eq!(bytes.used(), 17 + retained, "all temporary storage dropped");
        drop(storage);
        bytes.refund(retained);
        assert_eq!(bytes.used(), 17);
        let mut tight = Meter::new(retained, "survey memory");
        assert_eq!(
            survey(&build.scene, inputs(&build), &mut tight)
                .unwrap_err()
                .kind,
            ErrorKind::Limit
        );
        assert_eq!(tight.used(), 0, "failed survey retained no allocation");
    }

    #[test]
    fn survey_refunds_failed_walks_without_refunding_earlier_owners() {
        let mut build = Build::new(4.0, 4.0);
        let red = build.rgb([1.0, 0.0, 0.0]);
        let state = build.state(|state| state.soft_mask = Some(MaskId(99)));
        build.rect(GroupId(0), [0.0, 0.0, 4.0, 4.0], red, mark(state));
        let mut bytes = Meter::new(1 << 20, "survey memory");
        bytes.charge(19).unwrap();
        assert_eq!(
            survey(&build.scene, inputs(&build), &mut bytes)
                .unwrap_err()
                .kind,
            ErrorKind::InvalidArgument
        );
        assert_eq!(bytes.used(), 19);
        let mut empty = Meter::new(0, "survey memory");
        assert_eq!(
            survey(&build.scene, inputs(&build), &mut empty)
                .unwrap_err()
                .kind,
            ErrorKind::Limit
        );
        assert_eq!(empty.used(), 0);
    }
}
