//! Storage formats from whole content (ADR 0004 §5–§6), decided before any
//! region is considered: one pass over the page's content describes every
//! buffer (the page, each transparency group, soft mask and tiling cell) by
//! its stores, then formats are chosen from the innermost buffer outwards
//! and widened from the outermost inwards. The same pass decides which
//! groups composite as their members (flattening), which run from a
//! transparent start, and whether the page selects transfer functions.

use crate::format::Format;
use crate::plan::Layout;
use crate::prepare::index;
use pagura_color::Process;
use pagura_core::{Error, Result};
use pagura_interpret::{
    BlendMode, Composite, GroupAttributes, GroupId, GroupRole, MaskId, Node, Paint, PatternId,
    Scene,
};
use std::collections::HashMap;

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
    pub(crate) groups: HashMap<(GroupId, Process), Decision>,
    /// Soft masks by mask and parent blending space.
    pub(crate) masks: HashMap<(MaskId, Process), Format>,
    /// Whether the instances of a tiling pattern start transparent (Normal
    /// cell content), by pattern and blending space.
    pub(crate) cells: HashMap<(PatternId, Process), bool>,
    /// Some page content selects a transfer function (11.7.5.2).
    pub(crate) transfers: bool,
}

/// The inputs of a survey.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Inputs {
    /// The page's channels.
    pub(crate) page: Layout,
    /// Overprint simulation.
    pub(crate) simulate: bool,
    /// The `byte_storage` fast parameter.
    pub(crate) bytes: bool,
    /// The nesting budget.
    pub(crate) depth: usize,
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
        groups: HashMap::new(),
        masks: HashMap::new(),
        cells: HashMap::new(),
        open: Vec::new(),
        transfers: false,
    };
    let root = scene
        .group(scene.root)
        .ok_or(Error::invalid_argument("scene group out of range"))?;
    let page = survey.buffer(inputs.page, root.attributes.knockout);
    survey.buffers[page].page = true;
    survey.children(page, &root.children, Context::PAGE, 0)?;
    let page_knockout = survey.buffers[page].knockout();
    let transfers = survey.transfers;
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
    })
}

/// Where content is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Context {
    /// Page content, whose marks select transfer functions.
    selects: bool,
}

impl Context {
    const PAGE: Self = Self { selects: true };
    const OTHER: Self = Self { selects: false };
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
    groups: HashMap<(GroupId, Process), GroupEntry>,
    masks: HashMap<(MaskId, Process), usize>,
    cells: HashMap<(PatternId, Process), usize>,
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
            let (mark, kind, count) = match node {
                Node::Fill(fill) => (&fill.mark, self.paint(fill.paint)?, 1),
                Node::Stroke(stroke) => (&stroke.mark, self.paint(stroke.paint)?, 1),
                Node::Glyphs(glyphs) => (
                    &glyphs.mark,
                    self.paint(glyphs.paint)?,
                    glyphs.placements.len(),
                ),
                Node::Image(image) => (&image.mark, Kind::Varying, 1),
                Node::Shading(shading) => (&shading.mark, Kind::Varying, 1),
                Node::Group(group) => {
                    self.group(buffer, *group, context, depth)?;
                    continue;
                }
            };
            let state = state(scene, mark.state)?;
            self.mark(buffer, state, (kind, count), context, depth)?;
        }
        Ok(())
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
        (kind, count): (Kind, usize),
        context: Context,
        depth: usize,
    ) -> Result<()> {
        let layout = self.buffers[buffer].layout;
        let mask = state
            .soft_mask
            .map(|id| self.mask(id, layout.process, depth))
            .transpose()?;
        let cell = match kind {
            Kind::Pattern(pattern) => Some(self.cell(pattern, layout, depth)?),
            Kind::Constant | Kind::Varying => None,
        };
        if state.transfer.is_some() && context.selects {
            self.transfers = true;
        }
        let cell_reads = cell.is_some_and(|c| !self.buffers[c].normal_content());
        let target = &mut self.buffers[buffer];
        target.stores = target.stores.saturating_add(count);
        target.normal &= state.blend == BlendMode::Normal;
        target.forced |=
            state.blend != BlendMode::Normal || (state.transfer.is_some() && context.selects);
        let shaped = state.alpha_is_shape || (state.alpha == 1.0 && mask.is_none());
        target.opaque_members &= shaped && cell.is_none();
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
        if constant && mask.is_none() {
            if state.alpha > 0.0 {
                target.theta = target
                    .theta
                    .min(state.alpha / 255.0 * (1.0 - 1.0 / 4_194_304.0));
            }
            if state.alpha != 1.0 {
                target.dynamic = target.dynamic.saturating_add(count);
                target.eta += F64_STORE * count as f64;
            }
        } else {
            target.dynamic = target.dynamic.saturating_add(count);
            if kind == Kind::Varying {
                target.eta += MARGIN * count as f64;
            } else {
                target.eta += F64_STORE * count as f64;
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
        self.children(child, &group.children, context, depth + 1)?;
        let facts = self.buffers[child].clone();
        let plain =
            state.alpha == 1.0 && state.blend == BlendMode::Normal && state.soft_mask.is_none();
        let flatten = plain
            && !facts.knockout()
            && !self.buffers[parent].knockout_attribute
            && layout == parent_layout
            && (!group.attributes.isolated || facts.normal_content());
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
            .map(|mask| self.mask(mask, parent_layout.process, depth))
            .transpose()?;
        let buffer = &mut self.buffers[child];
        buffer.forced |= state.blend.amplifies() || facts.knockout() || !transparent;
        let target = &mut self.buffers[parent];
        target.stores = target.stores.saturating_add(1);
        target.dynamic = target.dynamic.saturating_add(1);
        target.normal &= state.blend == BlendMode::Normal;
        target.forced |= state.blend != BlendMode::Normal;
        target.opaque_members = false;
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

    fn mask(&mut self, id: MaskId, process: Process, depth: usize) -> Result<usize> {
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
        let walked = self.children(buffer, &group.children, Context::OTHER, depth + 1);
        self.open.pop();
        walked?;
        let facts = &mut self.buffers[buffer];
        let transparent = group.attributes.isolated || facts.normal_content();
        facts.forced |= mask.transfer.is_some() || facts.knockout() || !transparent;
        self.masks.insert(key, buffer);
        Ok(buffer)
    }

    fn cell(&mut self, pattern: PatternId, layout: Layout, depth: usize) -> Result<usize> {
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
        self.children(buffer, &group.children, Context::OTHER, depth + 1)?;
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
        let n16 = buffer.page && buffer.dynamic <= 1 && 257.0 * (buffer.eta + injected) <= N16_ROOM;
        let f32_bound =
            PI + RHO * stores.min(1.0 / buffer.theta + buffer.dynamic as f64) + injected;
        let (bound, format) = if buffer.forced {
            wide
        } else if n16 {
            // The page's bound is never used as a child's.
            (0.5, Format::N16)
        } else if f32_bound <= F32_LIMIT {
            (f32_bound + MARGIN, Format::F32)
        } else {
            wide
        };
        let format = if self.inputs.bytes && format != Format::F64 {
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
