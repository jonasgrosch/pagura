//! The recorded scene of a page (ADR 0034): device independent and policy
//! free, with marks in an arena in source order and interned paints, states
//! and clips. `pagura-paint` prepares and renders it; `pagura-reference`
//! renders it slowly and obviously correctly.

use crate::device::{OutlineKey, PageInfo};
use crate::model::{
    Composite, GroupAttributes, GroupCause, Image, MaskKind, Paint, Pen, Shading, TilingColors,
    TilingSpacing, ViewFit,
};
use pagura_color::{Components, Function};
use pagura_core::{Position, Rect};
use pagura_document::Rotation;
use pagura_raster::{FillRule, Path, Point, Transform};
use std::sync::Arc;

macro_rules! index {
    ($(#[$doc:meta] $name:ident),* $(,)?) => {$(
        #[$doc]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub u32);

        impl $name {
            /// The index as `usize`.
            pub fn index(self) -> usize {
                pagura_core::convert::u32_to_usize(self.0)
            }
        }
    )*};
}

index! {
    /// A node of [`Scene::nodes`]; node order is source order.
    NodeId,
    /// A group of [`Scene::groups`].
    GroupId,
    /// A clip of [`Scene::clips`].
    ClipId,
    /// A composite state of [`Scene::states`].
    StateId,
    /// A paint of [`Scene::paints`].
    PaintId,
    /// A pen of [`Scene::pens`].
    PenId,
}

/// The page geometry a scene is drawn in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageSpace {
    /// The crop box in default user space.
    pub crop: Rect,
    /// `Rotate`.
    pub rotation: Rotation,
    /// `UserUnit`.
    pub user_unit: f64,
}

impl PageSpace {
    /// The space of `page`.
    pub fn of(page: &PageInfo) -> Self {
        Self {
            crop: page.crop,
            rotation: page.rotation,
            user_unit: page.user_unit,
        }
    }
}

/// What every mark records besides its geometry and paint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarkInfo {
    /// Its compositing state.
    pub state: StateId,
    /// Its clip; `None` clips nothing but the page.
    pub clip: Option<ClipId>,
    /// The operator that painted it.
    pub position: Position,
}

/// A fill.
#[derive(Debug, Clone)]
pub struct FillNode {
    /// The path in default user space (pattern space in a tiling cell).
    pub path: Arc<Path>,
    /// The fill rule.
    pub rule: FillRule,
    /// The paint.
    pub paint: PaintId,
    /// State, clip and position.
    pub mark: MarkInfo,
}

/// A stroke.
#[derive(Debug, Clone)]
pub struct StrokeNode {
    /// The path in default user space.
    pub path: Arc<Path>,
    /// The pen.
    pub pen: PenId,
    /// The paint.
    pub paint: PaintId,
    /// State, clip and position.
    pub mark: MarkInfo,
}

/// A glyph outline under one linear map, shared by the placements of a run.
#[derive(Debug, Clone)]
pub struct GlyphSource {
    /// The outline in glyph space.
    pub outline: Arc<Path>,
    /// Its identity, for edge-setup caches.
    pub key: OutlineKey,
    /// Glyph space to default user space without translation.
    pub linear: Transform,
}

/// One glyph placement.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// Index into [`GlyphsNode::sources`].
    pub source: u32,
    /// The glyph origin in default user space.
    pub origin: Point,
}

/// Filled glyphs of one paint and state. Every placement is a separate mark
/// for compositing, overlapping pixels included (9.3.8).
#[derive(Debug, Clone)]
pub struct GlyphsNode {
    /// Distinct outline and linear-map pairs.
    pub sources: Vec<GlyphSource>,
    /// Placements in show order.
    pub placements: Vec<Placement>,
    /// The position of each placement from its index on: `(first, position)`
    /// pairs with increasing `first`.
    pub positions: Vec<(u32, Position)>,
    /// The paint.
    pub paint: PaintId,
    /// State, clip and the run's position.
    pub mark: MarkInfo,
}

impl GlyphsNode {
    /// The position of placement `index`.
    pub fn position(&self, index: usize) -> Position {
        let count = self
            .positions
            .partition_point(|&(first, _)| pagura_core::convert::u32_to_usize(first) <= index);
        count
            .checked_sub(1)
            .and_then(|i| self.positions.get(i))
            .map_or(self.mark.position, |&(_, position)| position)
    }
}

/// An image or stencil mask.
#[derive(Debug, Clone)]
pub struct ImageNode {
    /// The image.
    pub image: Arc<Image>,
    /// Unit square to default user space.
    pub transform: Transform,
    /// The paint of a stencil mask.
    pub stencil: Option<PaintId>,
    /// State, clip and position.
    pub mark: MarkInfo,
}

/// A shading painted with `sh`.
#[derive(Debug, Clone)]
pub struct ShadingNode {
    /// The shading.
    pub shading: Arc<Shading>,
    /// Shading space to default user space.
    pub transform: Transform,
    /// State, clip and position.
    pub mark: MarkInfo,
}

/// A node of the scene.
#[derive(Debug, Clone)]
pub enum Node {
    /// A fill.
    Fill(FillNode),
    /// A stroke.
    Stroke(StrokeNode),
    /// Filled glyphs.
    Glyphs(GlyphsNode),
    /// An image.
    Image(ImageNode),
    /// A shading.
    Shading(ShadingNode),
    /// A transparency group, painted where it occurs.
    Group(GroupId),
}

/// What a group is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupRole {
    /// The page group, the root of the scene.
    Page,
    /// A transparency group painted where it occurs.
    Transparency,
    /// The group of a soft mask.
    SoftMask {
        /// Alpha or luminosity.
        kind: MaskKind,
    },
    /// The cell of a tiling pattern (not a transparency group: its marks
    /// composite into the pattern's use).
    Tiling,
}

/// A group of nodes: the page, a transparency group, a soft mask's group or
/// a tiling cell.
#[derive(Debug, Clone)]
pub struct Group {
    /// What it is.
    pub role: GroupRole,
    /// Why it exists.
    pub cause: GroupCause,
    /// Its attributes.
    pub attributes: GroupAttributes,
    /// Its bounding box in default user space (pattern space for a cell):
    /// a bound for allocating its buffer, never the only carrier of its
    /// clip. A form's, cell's or soft mask's `BBox` clips its marks through
    /// their clip chains, which include it.
    pub bbox: Option<Arc<Path>>,
    /// A fixed view, for annotations with `NoZoom` or `NoRotate`.
    pub view: Option<ViewFit>,
    /// The nodes inside, in source order.
    pub children: Vec<NodeId>,
    /// The state, clip and position its result is composited with (unused
    /// for the page, soft masks and cells).
    pub mark: MarkInfo,
}

/// A soft mask.
#[derive(Debug, Clone)]
pub struct SoftMask {
    /// Its group.
    pub group: GroupId,
    /// Alpha or luminosity.
    pub kind: MaskKind,
    /// `BC` in the group's blending space.
    pub backdrop: Option<Components>,
    /// `TR`.
    pub transfer: Option<Arc<Function>>,
    /// The `gs` that set it.
    pub position: Position,
}

/// A tiling pattern.
#[derive(Debug, Clone)]
pub struct Tiling {
    /// Its cell.
    pub cell: GroupId,
    /// `BBox` in pattern space.
    pub bbox: Rect,
    /// `XStep` and `YStep`.
    pub step: [f64; 2],
    /// Pattern space to default user space.
    pub matrix: Transform,
    /// `PaintType`.
    pub colors: TilingColors,
    /// `TilingType`.
    pub spacing: TilingSpacing,
}

/// A clip in the scene's interned chain.
#[derive(Debug, Clone)]
pub struct ClipNode {
    /// The enclosing clip.
    pub parent: Option<ClipId>,
    /// The clipping path in default user space.
    pub path: Arc<Path>,
    /// The fill rule.
    pub rule: FillRule,
}

/// A page recorded for rendering.
///
/// Every id a node holds indexes the matching table; the recorder
/// guarantees it, so lookups by id cannot fail for a scene it built.
#[derive(Debug, Clone)]
pub struct Scene {
    /// The page geometry.
    pub page: PageSpace,
    /// Whether the document declares a PDF/X print intent.
    pub print_intent: bool,
    /// The page group.
    pub root: GroupId,
    /// Every node in source order.
    pub nodes: Vec<Node>,
    /// Groups.
    pub groups: Vec<Group>,
    /// Soft masks, by `MaskId`.
    pub masks: Vec<SoftMask>,
    /// Tiling patterns, by `PatternId`.
    pub tilings: Vec<Tiling>,
    /// Clips.
    pub clips: Vec<ClipNode>,
    /// Composite states.
    pub states: Vec<Arc<Composite>>,
    /// Paints.
    pub paints: Vec<Arc<Paint>>,
    /// Pens.
    pub pens: Vec<Arc<Pen>>,
    /// Bytes the scene retains, for the `Store`: table capacities and owned
    /// payload estimates, excluding recording indexes and scratch. Shared
    /// payloads may be counted per retained owner; this is not allocator
    /// telemetry and does not depend on Store or reference-count state.
    pub bytes: usize,
}

impl Scene {
    /// The node `id`.
    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(id.index())
    }

    /// The group `id`.
    pub fn group(&self, id: GroupId) -> Option<&Group> {
        self.groups.get(id.index())
    }

    /// The number of painted marks, counting each glyph placement.
    pub fn marks(&self) -> usize {
        self.nodes
            .iter()
            .map(|node| match node {
                Node::Glyphs(glyphs) => glyphs.placements.len(),
                Node::Group(_) => 0,
                _ => 1,
            })
            .sum()
    }
}
