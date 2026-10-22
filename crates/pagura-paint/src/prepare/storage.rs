//! Storage formats from whole content (ADR 0004 §5–§6), decided before any
//! region is considered: one pass over the page's content describes every
//! buffer (the page, each transparency group, soft mask and tiling cell) by
//! its stores, then formats are chosen from the innermost buffer outwards
//! and widened from the outermost inwards. The same pass decides which
//! groups composite as their members (flattening), which run from a
//! transparent start, and whether the page selects transfer functions.

use crate::format::Format;
use crate::plan::Layout;
use crate::prepare::bounds;
use crate::prepare::index;
use crate::prepare::overprint::{Keeping, Place};
use pagura_color::Process;
use pagura_core::{Error, Result};
use pagura_interpret::{
    BlendMode, Composite, GroupAttributes, GroupId, GroupRole, ImageMask, MaskId, Node, Paint,
    PatternId, Scene,
};
use pagura_raster::{DeviceRect, Path, Transform};
use crate::hash::{self, FastMap};

/// `π` of the F32 bound: source encoding and six roundings relative to the
/// opacity, in levels.
const PI: f64 = 1.0 / 65_536.0 + 255.0 * 6.0 / 16_777_216.0;
/// `ρ` of the F32 bound: one rounding below 256 levels.
const RHO: f64 = 1.0 / 131_072.0;
/// The margin for f64 source evaluation, in levels.
const MARGIN: f64 = 1.0 / 4096.0;
/// The error of one f64 store of a non-amplifying operation, in levels.
const F64_STORE: f64 = 1.0 / 1_099_511_627_776.0;
/// The F32 admission limit, in levels.
const F32_LIMIT: f64 = 0.5 - 1.0 / 4096.0;
/// The room an N16 buffer leaves for dynamic stores, in words.
const N16_ROOM: f64 = 0.5 - 1.0 / 68_719_476_736.0;

/// What preparation needs to know about one transparency group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Decision {
    /// Its members composite into the parent directly.
    pub(crate) flatten: bool,
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
    /// outside soft masks are then unsupported, as before.
    pub(crate) transfer_gate: bool,
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
    /// The `byte_storage` fast parameter.
    pub(crate) bytes: bool,
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
/// A `Limit` error for nesting beyond `inputs.depth`, `Syntax` for a soft
/// mask whose content uses itself and `InvalidArgument` for ids out of
/// range.
pub(crate) fn survey(scene: &Scene, inputs: Inputs) -> Result<Storage> {
    let mut survey = Survey {
        scene,
        inputs,
        buffers: Vec::new(),
        groups: hash::map(),
        masks: hash::map(),
        cells: hash::map(),
        open: Vec::new(),
        transfers: false,
    };
    let root = scene
        .group(scene.root)
        .ok_or(Error::invalid_argument("scene group out of range"))?;
    let page = survey.buffer(inputs.page, root.attributes.knockout);
    survey.buffers[page].page = true;
    survey.children(page, &root.children, Context::page(inputs.transform), 0)?;
    let page_knockout = survey.buffers[page].knockout();
    let transfers = survey.transfers;
    let transfer_gate = survey.buffers[page].transfer;
    let buffer = &mut survey.buffers[page];
    buffer.forced |=
        inputs.simulate || inputs.page.process != Process::Rgb || page_knockout || transfers;
    let mut resolved = vec![None; survey.buffers.len()];
    survey.resolve(page, &mut resolved, 0)?;
    survey.widen(page, &mut resolved);
    let format = |node: usize| {
        resolved
            .get(node)
            .copied()
            .flatten()
            .map_or(Format::F64, |(format, _)| format)
    };
    let groups = survey
        .groups
        .iter()
        .map(|(&key, entry)| {
            let decision = Decision {
                flatten: entry.flatten,
                format: format(entry.buffer),
                transparent: entry.transparent,
                knockout: entry.knockout,
            };
            (key, decision)
        })
        .collect();
    let masks = survey
        .masks
        .iter()
        .map(|(&key, &node)| (key, format(node)))
        .collect();
    let cells = survey
        .cells
        .iter()
        .map(|(&key, &node)| (key, survey.buffers[node].normal_content()))
        .collect();
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

/// Where content is.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Context {
    /// Page content, whose marks select transfer functions.
    selects: bool,
    /// Soft-mask content.
    masked: bool,
    /// Content space to device pixels.
    transform: Transform,
}

impl Context {
    fn page(transform: Transform) -> Self {
        Self {
            selects: true,
            masked: false,
            transform,
        }
    }

    fn mask(self) -> Self {
        Self {
            selects: false,
            masked: true,
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

/// The most dynamic stores whose footprints N16 admission compares.
const FOOTPRINTS: usize = 4096;

/// Whether an N16 page admits `buffer`'s dynamic stores: at most one at
/// any pixel, each injecting less than the room an N16 pixel leaves
/// (ADR 0004 §6). Footprints are page-level device bounds, so the answer
/// does not depend on the region. Children and masks are resolved first.
fn n16_admits(buffer: &Buffer, resolved: &[Option<(Format, f64)>]) -> bool {
    if buffer.dynamics.len() != buffer.dynamic || buffer.dynamic > FOOTPRINTS {
        return false;
    }
    let bound = |node: usize| {
        resolved
            .get(node)
            .copied()
            .flatten()
            .map_or(f64::INFINITY, |(_, bound)| bound)
    };
    let fits = buffer.dynamics.iter().all(|store| {
        let child = store
            .child
            .map_or(0.0, |c| bound(c) * (1.0 + 256.0 / 255.0) + F64_STORE);
        let mask = store.mask.map_or(0.0, |m| 256.0 / 255.0 * bound(m));
        257.0 * (store.eta + child + mask) <= N16_ROOM
    });
    let mut rects: Vec<DeviceRect> = buffer
        .dynamics
        .iter()
        .map(|store| store.rect)
        .filter(|rect| !rect.is_empty())
        .collect();
    fits && disjoint(&mut rects)
}

/// Whether no two rectangles share a pixel.
fn disjoint(rects: &mut [DeviceRect]) -> bool {
    rects.sort_unstable_by_key(|r| r.x0);
    let mut active: Vec<DeviceRect> = Vec::new();
    for rect in rects.iter() {
        active.retain(|other| other.x1 > rect.x0);
        if active
            .iter()
            .any(|other| other.y0 < rect.y1 && rect.y0 < other.y1)
        {
            return false;
        }
        active.push(*rect);
    }
    true
}

/// A buffer described by its stores.
#[derive(Debug, Clone)]
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
    /// Every store blends Normal.
    normal: bool,
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
    fn knockout(&self) -> bool {
        self.knockout_attribute && !self.opaque_members
    }

    /// Normal content: compositing it onto a backdrop equals compositing
    /// it alone and then onto the backdrop.
    fn normal_content(&self) -> bool {
        self.normal && !self.knockout() && !self.reads
    }

    fn merge(&mut self, other: &Self) {
        self.forced |= other.forced;
        self.opaque_members &= other.opaque_members;
        self.normal &= other.normal;
        self.reads |= other.reads;
        self.stores = self.stores.saturating_add(other.stores);
        self.dynamic = self.dynamic.saturating_add(other.dynamic);
        self.theta = self.theta.min(other.theta);
        self.eta += other.eta;
        self.children.extend_from_slice(&other.children);
        self.masks.extend_from_slice(&other.masks);
        self.dynamics.extend_from_slice(&other.dynamics);
        self.extent = unite(self.extent, other.extent);
        self.transfer |= other.transfer;
    }
}

#[derive(Debug, Clone, Copy)]
struct GroupEntry {
    buffer: usize,
    flatten: bool,
    transparent: bool,
    knockout: bool,
}

struct Survey<'s> {
    scene: &'s Scene,
    inputs: Inputs,
    buffers: Vec<Buffer>,
    groups: FastMap<(GroupId, Process), GroupEntry>,
    masks: FastMap<(MaskId, Process), usize>,
    cells: FastMap<(PatternId, Process), usize>,
    open: Vec<(MaskId, Process)>,
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
    fn buffer(&mut self, layout: Layout, knockout: bool) -> usize {
        self.buffers.push(Buffer {
            layout,
            page: false,
            forced: self.inputs.simulate || layout.process != Process::Rgb,
            knockout_attribute: knockout,
            opaque_members: true,
            normal: true,
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
        self.buffers.len() - 1
    }

    fn children(
        &mut self,
        buffer: usize,
        children: &[pagura_interpret::NodeId],
        context: Context,
        depth: usize,
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
                    self.group(buffer, *group, context, depth)?;
                    continue;
                }
            };
            let state = state(scene, mark.state)?;
            // Footprints matter for dynamic stores and for the extent of a
            // group, whose result is a dynamic store of its parent.
            let hulls = if dynamic(kind, state) || !self.buffers[buffer].page {
                hulls(scene, node, context.transform)?
            } else {
                Vec::new()
            };
            let stores = Stores {
                kind,
                count,
                hulls: &hulls,
                alpha_shape,
            };
            self.mark(buffer, state, &stores, context, depth)?;
        }
        self.settle(buffer);
        Ok(())
    }

    /// Flattens the pending groups of `buffer` once its content shows that
    /// its knockout is the opaque-member identity, so that it composites as
    /// source-over and they can composite straight into it.
    fn settle(&mut self, buffer: usize) {
        let pending = std::mem::take(&mut self.buffers[buffer].pending);
        if self.buffers[buffer].knockout() {
            return;
        }
        for (key, child) in pending {
            let facts = self.buffers[child].clone();
            let target = &mut self.buffers[buffer];
            // Its result was one dynamic store.
            target.stores = target.stores.saturating_sub(1);
            target.dynamic = target.dynamic.saturating_sub(1);
            target.dynamics.retain(|store| store.child != Some(child));
            target.children.retain(|&(c, _)| c != child);
            target.merge(&facts);
            if let Some(entry) = self.groups.get_mut(&key) {
                entry.flatten = true;
            }
        }
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
    ) -> Result<()> {
        let (kind, count) = (stores.kind, stores.count);
        let layout = self.buffers[buffer].layout;
        let mask = state
            .soft_mask
            .map(|id| self.mask(id, layout.process, (context, depth)))
            .transpose()?;
        let cell = match kind {
            Kind::Pattern(pattern) => Some(self.cell(pattern, layout, (context, depth))?),
            Kind::Constant | Kind::Varying => None,
        };
        let rects: Vec<DeviceRect> = stores.hulls.iter().map(|&hull| self.pixels(hull)).collect();
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
        target.stores = target.stores.saturating_add(count);
        target.transfer |= transfer;
        // A store that keeps channels reads its backdrop.
        target.normal &= state.blend == BlendMode::Normal && !keeps;
        // A store with a transfer function is outside the narrow bounds
        // wherever it is (ADR 0004 §5).
        target.forced |= state.blend != BlendMode::Normal || state.transfer.is_some();
        for &hull in stores.hulls {
            target.extent = unite(target.extent, hull);
        }
        // Opacity equals shape, and a knockout of such stores is source-over
        // only when they blend Normal and keep no channel of the backdrop.
        let shaped =
            stores.alpha_shape && (state.alpha_is_shape || (state.alpha == 1.0 && mask.is_none()));
        target.opaque_members &=
            shaped && cell.is_none() && state.blend == BlendMode::Normal && !keeps;
        if cell_reads {
            // A cell whose content reads its backdrop reads this buffer.
            target.reads = true;
            target.forced = true;
        }
        if let Some(mask) = mask {
            target.masks.push((mask, count));
        }
        if let Some(cell) = cell {
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
                target.dynamics.extend(rects.iter().map(|&rect| Dynamic {
                    rect,
                    eta,
                    child: cell,
                    mask,
                }));
            }
        }
        Ok(())
    }

    fn group(&mut self, parent: usize, id: GroupId, context: Context, depth: usize) -> Result<()> {
        let scene = self.scene;
        let group = scene
            .group(id)
            .ok_or(Error::invalid_argument("scene group out of range"))?;
        if group.role != GroupRole::Transparency {
            return Ok(());
        }
        let state = state(scene, group.mark.state)?;
        let parent_layout = self.buffers[parent].layout;
        let layout = group_layout(group.attributes, parent_layout);
        let child = self.buffer(layout, group.attributes.knockout);
        let inner = match group.view {
            Some(view) => Context {
                transform: crate::prepare::planner::view_transform(
                    context.transform,
                    view,
                    self.inputs.user_unit,
                )?,
                ..context
            },
            None => context,
        };
        self.children(child, &group.children, inner, depth + 1)?;
        let facts = self.buffers[child].clone();
        let plain =
            state.alpha == 1.0 && state.blend == BlendMode::Normal && state.soft_mask.is_none();
        let fits = plain
            && !facts.knockout()
            && layout == parent_layout
            && (!group.attributes.isolated || facts.normal_content());
        let parent_knockout = self.buffers[parent].knockout_attribute;
        let flatten = fits && !parent_knockout;
        // In a knockout group, a plain group of opaque members is one opaque
        // member: the implicit groups of text objects and of fill-and-stroke
        // operators around Type 3 glyphs or fill-and-stroke text.
        let pending = fits && parent_knockout && facts.opaque_members;
        let transparent = group.attributes.isolated || facts.normal_content();
        self.groups.insert(
            (id, parent_layout.process),
            GroupEntry {
                buffer: child,
                flatten,
                transparent,
                knockout: facts.knockout(),
            },
        );
        if flatten {
            self.buffers[parent].merge(&facts);
            return Ok(());
        }
        let mask = state
            .soft_mask
            .map(|mask| self.mask(mask, parent_layout.process, (context, depth)))
            .transpose()?;
        let buffer = &mut self.buffers[child];
        buffer.forced |= state.blend.amplifies() || facts.knockout() || !transparent;
        let rect = self.pixels(facts.extent);
        let target = &mut self.buffers[parent];
        target.extent = unite(target.extent, facts.extent);
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
        target.transfer |= facts.transfer;
        target.opaque_members &= pending;
        if pending {
            target.pending.push(((id, parent_layout.process), child));
        }
        target.children.push((child, 1));
        if let Some(mask) = mask {
            target.masks.push((mask, 1));
        }
        if !transparent {
            target.reads = true;
            target.forced = true;
        }
        Ok(())
    }

    fn mask(
        &mut self,
        id: MaskId,
        process: Process,
        (context, depth): (Context, usize),
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
        let buffer = self.buffer(layout, group.attributes.knockout);
        self.open.push(key);
        let walked = self.children(buffer, &group.children, context.mask(), depth + 1);
        self.open.pop();
        walked?;
        let facts = &mut self.buffers[buffer];
        let transparent = group.attributes.isolated || facts.normal_content();
        facts.forced |= mask.transfer.is_some() || facts.knockout() || !transparent;
        self.masks.insert(key, buffer);
        Ok(buffer)
    }

    fn cell(
        &mut self,
        pattern: PatternId,
        layout: Layout,
        (context, depth): (Context, usize),
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
        let buffer = self.buffer(layout, false);
        // Registered before its content: a cell painting itself is caught by
        // the nesting budget, as its instances nest without end.
        self.cells.insert((pattern, layout.process), buffer);
        let inner = Context {
            selects: false,
            transform: tiling.matrix.then(context.transform),
            ..context
        };
        self.children(buffer, &group.children, inner, depth + 1)?;
        self.buffers[buffer].forced = true;
        Ok(buffer)
    }

    /// Chooses formats from the innermost buffer outwards and records each
    /// buffer's error bound in levels.
    fn resolve(
        &self,
        node: usize,
        resolved: &mut Vec<Option<(Format, f64)>>,
        depth: usize,
    ) -> Result<(Format, f64)> {
        if let Some(Some(done)) = resolved.get(node) {
            return Ok(*done);
        }
        if depth > self.inputs.depth.saturating_mul(2) {
            return Err(Error::limit("compositing depth exceeded"));
        }
        let buffer = &self.buffers[node];
        let mut injected = 0.0;
        for &(child, count) in &buffer.children {
            let (_, bound) = self.resolve(child, resolved, depth + 1)?;
            injected += (bound * (1.0 + 256.0 / 255.0) + F64_STORE) * count as f64;
        }
        for &(mask, count) in &buffer.masks {
            let (_, bound) = self.resolve(mask, resolved, depth + 1)?;
            injected += 256.0 / 255.0 * bound * count as f64;
        }
        let stores = buffer.stores as f64;
        let wide = (stores * F64_STORE + MARGIN + injected, Format::F64);
        let n16 = buffer.page && n16_admits(buffer, resolved);
        let f32_bound =
            PI + RHO * stores.min(1.0 / buffer.theta + buffer.dynamic as f64) + injected;
        // Byte storage narrows only a page without group results, tiling
        // cells or soft masks; everything else keeps f64, as the earlier
        // competitive renderer kept it.
        let bytes = buffer.page && buffer.children.is_empty() && buffer.masks.is_empty();
        let (bound, format) = if buffer.forced || (self.inputs.bytes && !bytes) {
            wide
        } else if n16 {
            // The page's bound is never used as a child's.
            (0.5, Format::N16)
        } else if f32_bound <= F32_LIMIT {
            (f32_bound + MARGIN, Format::F32)
        } else {
            wide
        };
        let format = if self.inputs.bytes && bytes && format != Format::F64 {
            Format::U8
        } else {
            format
        };
        if let Some(slot) = resolved.get_mut(node) {
            *slot = Some((format, bound));
        }
        Ok((format, bound))
    }

    /// Widens every child and mask of an F64 buffer to F64, from the
    /// outermost buffer inwards: no narrow child of an F64 parent is proven.
    fn widen(&self, root: usize, resolved: &mut [Option<(Format, f64)>]) {
        // 1: visited while narrow, 2: visited while F64.
        let mut seen = vec![0_u8; self.buffers.len()];
        let mut pending = vec![(root, false)];
        while let Some((node, parent_wide)) = pending.pop() {
            let Some(Some((format, _))) = resolved.get_mut(node) else {
                continue;
            };
            if parent_wide {
                *format = Format::F64;
            }
            let wide = *format == Format::F64;
            let mark = if wide { 2 } else { 1 };
            if seen[node] >= mark {
                continue;
            }
            seen[node] = mark;
            let buffer = &self.buffers[node];
            for &(child, _) in buffer.children.iter().chain(&buffer.masks) {
                pending.push((child, wide));
            }
        }
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

/// A device rectangle that covers every pixel.
const WHOLE: bounds::Hull = [f64::MIN, f64::MIN, f64::MAX, f64::MAX];

/// The smallest hull holding both.
fn unite(a: Option<bounds::Hull>, b: Option<bounds::Hull>) -> Option<bounds::Hull> {
    match (a, b) {
        (Some(a), Some(b)) => Some([
            a[0].min(b[0]),
            a[1].min(b[1]),
            a[2].max(b[2]),
            a[3].max(b[3]),
        ]),
        (a, None) => a,
        (None, b) => b,
    }
}

/// The stores of one node.
struct Stores<'h> {
    kind: Kind,
    /// Stores: one for most marks, one per placement for glyphs.
    count: usize,
    /// One device hull per store where footprints matter, else none.
    hulls: &'h [Option<bounds::Hull>],
    /// The source's alpha is its shape (not an image with a soft mask).
    alpha_shape: bool,
}

/// Whether a store of `kind` under `state` is dynamic (ADR 0004 §6): all
/// but opaque constant colours that only coverage reduces.
fn dynamic(kind: Kind, state: &Composite) -> bool {
    !(kind == Kind::Constant && state.soft_mask.is_none() && state.alpha == 1.0)
}

/// The device hulls of a node's stores.
fn hulls(scene: &Scene, node: &Node, t: Transform) -> Result<Vec<Option<bounds::Hull>>> {
    Ok(match node {
        Node::Fill(fill) => vec![fill_hull(fill, t)],
        Node::Stroke(stroke) => vec![stroke_hull(scene, stroke, t)?],
        Node::Glyphs(glyphs) => glyph_hulls(glyphs, t),
        Node::Image(image) => vec![bounds::path(
            &bounds::unit_square(),
            image.transform.then(t),
        )],
        Node::Shading(shading) => vec![shading.shading.bbox.map_or(Some(WHOLE), |b| {
            bounds::path(&Path::rect(b), shading.transform.then(t))
        })],
        Node::Group(_) => Vec::new(),
    })
}

/// A fill's device hull, a pixel wider for degenerate subpaths.
fn fill_hull(fill: &pagura_interpret::FillNode, transform: Transform) -> Option<bounds::Hull> {
    bounds::path(&fill.path, transform).map(|h| bounds::expand(h, 1.0))
}

/// A stroke's device hull: its path's, grown by the pen's reach.
fn stroke_hull(
    scene: &Scene,
    stroke: &pagura_interpret::StrokeNode,
    transform: Transform,
) -> Result<Option<bounds::Hull>> {
    let pen = scene
        .pens
        .get(stroke.pen.index())
        .ok_or(Error::invalid_argument("scene pen out of range"))?;
    Ok(bounds::path(&stroke.path, transform)
        .map(|h| bounds::expand(h, bounds::stroke_reach(pen, transform) + 2.0)))
}

/// The device hull of every placement of a glyph run.
fn glyph_hulls(
    glyphs: &pagura_interpret::GlyphsNode,
    transform: Transform,
) -> Vec<Option<bounds::Hull>> {
    let t = transform;
    let device = Transform::new(t.a, t.b, t.c, t.d, 0.0, 0.0);
    let relative: Vec<Option<bounds::Hull>> = glyphs
        .sources
        .iter()
        .map(|source| {
            let l = source.linear;
            bounds::path(
                &source.outline,
                Transform::new(l.a, l.b, l.c, l.d, 0.0, 0.0).then(device),
            )
        })
        .collect();
    glyphs
        .placements
        .iter()
        .map(|placement| {
            let origin = t.apply(placement.origin);
            relative
                .get(index(placement.source))
                .copied()
                .flatten()
                .map(|[x0, y0, x1, y1]| {
                    bounds::expand(
                        [x0 + origin.x, y0 + origin.y, x1 + origin.x, y1 + origin.y],
                        1.0,
                    )
                })
        })
        .collect()
}
