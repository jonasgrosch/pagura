//! The prepared form of a scene for one request: groups with their buffer
//! layouts and formats, items (marks and group results) in source order
//! with their band ranges, soft masks, prepared paints and output. Built by
//! preparation, read-only during execution.

use crate::format::Format;
use crate::paint::{Convert, Curve, PaintSpans};
use pagura_color::Process;
use pagura_core::Position;
use pagura_interpret::{BlendMode, MaskKind};
use pagura_raster::{Area, DeviceRect};
use std::sync::Arc;

/// The most rows of one band. Band buffers, scratch storage and their
/// charges are sized for `min(region height, MAX_BAND_HEIGHT)` rows
/// whatever band height a request asks for, so charges do not depend on it.
pub(crate) const MAX_BAND_HEIGHT: u32 = 256;

/// The most spot colorant channels overprint simulation keeps (ADR 0004).
pub(crate) const MAX_SPOTS: usize = 32;

/// The channels of a buffer: process components, then spot colorants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Layout {
    /// The blending space.
    pub(crate) process: Process,
    /// Spot colorant channels.
    pub(crate) spots: usize,
}

impl Layout {
    /// Colour channels.
    pub(crate) fn channels(self) -> usize {
        self.process.components() + self.spots
    }

    /// Values per f64 pixel: colour channels (premultiplied), alpha, group
    /// alpha and shape.
    pub(crate) fn stride(self) -> usize {
        self.channels() + 3
    }

    /// Every colour channel, as overprint bits.
    pub(crate) fn mask(self) -> u64 {
        let channels = self.channels();
        if channels >= 64 {
            u64::MAX
        } else {
            (1_u64 << channels) - 1
        }
    }
}

/// Everything the executor needs.
#[derive(Debug)]
pub(crate) struct Plan {
    /// The region and the band grid of execution.
    pub(crate) area: Area,
    /// Groups; the page group is the first.
    pub(crate) groups: Vec<GroupPlan>,
    /// Items of every group.
    pub(crate) items: Vec<Item>,
    /// Soft masks.
    pub(crate) masks: Vec<MaskPlan>,
    /// Prepared paints.
    pub(crate) paints: Vec<Box<dyn PaintSpans>>,
    /// Process conversions of group results.
    pub(crate) conversions: Vec<Conversion>,
    /// Output.
    pub(crate) output: Output,
    /// The stacks of group buffers alive at once.
    pub(crate) stack: crate::execute::Tops,
}

/// A buffer: the page, a transparency group, a soft mask's group or the
/// instances of a tiling pattern.
#[derive(Debug)]
pub(crate) struct GroupPlan {
    /// The pixels the buffer covers, within the region.
    pub(crate) bounds: DeviceRect,
    /// Item ids in source order.
    pub(crate) items: Vec<u32>,
    /// Positions in `items`, ordered by first band and then source order.
    pub(crate) order: Vec<u32>,
    /// Channels.
    pub(crate) layout: Layout,
    /// Storage format.
    pub(crate) format: Format,
    /// `K`: members replace each other by shape (11.4.6).
    pub(crate) knockout: bool,
    /// What the buffer starts as.
    pub(crate) start: Start,
}

/// The initial backdrop of a group buffer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Start {
    /// Transparent: an isolated group, an alpha mask, the page.
    Transparent,
    /// The parent's backdrop for its children: a non-isolated group or the
    /// instances of a tiling pattern (11.4.4).
    Backdrop,
    /// An opaque colour: the `BC` of a non-isolated luminosity mask.
    Opaque([f64; 4]),
}

/// A mark or group result composited into its group, in source order.
#[derive(Debug)]
pub(crate) struct Item {
    /// The pixels it can change.
    pub(crate) bounds: DeviceRect,
    /// The first and last band it meets.
    pub(crate) bands: [u32; 2],
    /// Its place in preparation order.
    pub(crate) serial: u32,
    /// The operator that painted it.
    pub(crate) position: Position,
    /// Rasterization work reserved for its coverage.
    pub(crate) work: usize,
    /// What it is.
    pub(crate) kind: ItemKind,
}

/// The kind of an item.
#[derive(Debug)]
pub(crate) enum ItemKind {
    /// A mark.
    Mark(Mark),
    /// A group result.
    Group(GroupUse),
}

/// A mark's coverage: its shape and clip in the coverage producer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Cover {
    /// The shape; `None` covers the whole area.
    pub(crate) shape: Option<u32>,
    /// The clip state; `None` keeps everything.
    pub(crate) clip: Option<u32>,
}

/// A fill, stroke, glyph placement, image or shading.
#[derive(Debug)]
pub(crate) struct Mark {
    /// Coverage.
    pub(crate) cover: Cover,
    /// Index into [`Plan::paints`].
    pub(crate) paint: u32,
    /// Compositing.
    pub(crate) compositing: Compositing,
    /// The transfer selection it records (11.7.5.2); 0 is the default.
    pub(crate) select: u16,
    /// Whether it records a transfer selection at all: page content only.
    pub(crate) records: bool,
    /// The shape whose coverage a shading pattern mixes its `Background`
    /// by ([`crate::source::BBoxUse::Boundary`]).
    pub(crate) boundary: Option<u32>,
}

/// A group's result composited into its parent.
#[derive(Debug)]
pub(crate) struct GroupUse {
    /// The group.
    pub(crate) group: u32,
    /// The coverage of a pattern-filled mark; `None` for a transparency
    /// group, whose result carries its own shape.
    pub(crate) cover: Option<Cover>,
    /// Compositing.
    pub(crate) compositing: Compositing,
    /// Index into [`Plan::conversions`] when the group's blending space is
    /// not its parent's.
    pub(crate) conversion: Option<u32>,
}

/// How an item composites (11.3, 11.6.4, 11.7.4).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Compositing {
    /// Constant alpha.
    pub(crate) alpha: f64,
    /// Blend mode.
    pub(crate) blend: BlendMode,
    /// `AIS`.
    pub(crate) alpha_is_shape: bool,
    /// Index into [`Plan::masks`].
    pub(crate) mask: Option<u32>,
    /// Channels that keep the backdrop where the paint does not paint them:
    /// every channel for a simulated overprinting mark, none otherwise. The
    /// paint's channels follow its overprint state, zero DeviceCMYK
    /// components under `OPM 1` excluded (8.6.7).
    pub(crate) overprint: u64,
}

impl Compositing {
    /// Whether only coverage reduces the source: alpha 1, Normal, no mask,
    /// nothing kept.
    pub(crate) fn is_opaque(&self) -> bool {
        self.alpha == 1.0
            && self.blend == BlendMode::Normal
            && self.mask.is_none()
            && self.overprint == 0
    }
}

/// A soft mask prepared for one transform and parent blending space.
#[derive(Debug)]
pub(crate) struct MaskPlan {
    /// Its group.
    pub(crate) group: u32,
    /// Alpha or luminosity.
    pub(crate) kind: MaskKind,
    /// `I` of its group.
    pub(crate) isolated: bool,
    /// `BC` in the group's blending space, additively.
    pub(crate) backdrop: [f64; 4],
    /// `TR`.
    pub(crate) transfer: Option<Arc<dyn Curve>>,
    /// Where values are computed: its group's content within the pixels
    /// that use it.
    pub(crate) extent: DeviceRect,
    /// The value everywhere else.
    pub(crate) outside: f64,
}

/// A conversion of a group result into its parent's blending space.
#[derive(Debug)]
pub(crate) struct Conversion {
    /// The group's process space.
    pub(crate) from: Process,
    /// The parent's.
    pub(crate) to: Process,
    /// `BG` of the group's composite state.
    pub(crate) black: Option<Arc<dyn Curve>>,
    /// `UCR` of the group's composite state.
    pub(crate) removal: Option<Arc<dyn Curve>>,
}

/// The page buffer's conversion into RGBA8 (11.3.3).
#[derive(Debug)]
pub(crate) struct Output {
    /// The page's channels.
    pub(crate) layout: Layout,
    /// The background, premultiplied once.
    pub(crate) background: [u8; 4],
    /// Spot colorant equivalents, by first painting definition.
    pub(crate) spots: Vec<SpotPlan>,
    /// Transfer functions by selection, from selection 1 on: per RGB
    /// channel, `None` the identity.
    pub(crate) transfers: Vec<[Option<Arc<dyn Curve>>; 3]>,
}

/// One colorant definition that supplies the display equivalent of spot
/// channels (overprint preview, ADR 0004).
#[derive(Debug)]
pub(crate) struct SpotPlan {
    /// Its alternate space into CMYK.
    pub(crate) convert: Box<dyn Convert>,
    /// Its components.
    pub(crate) components: usize,
    /// `(component, spot channel)` pairs.
    pub(crate) inputs: Vec<(usize, usize)>,
}

/// The intersection of two rectangles; empty when they do not meet.
pub(crate) fn intersect(a: DeviceRect, b: DeviceRect) -> DeviceRect {
    DeviceRect {
        x0: a.x0.max(b.x0),
        y0: a.y0.max(b.y0),
        x1: a.x1.min(b.x1),
        y1: a.y1.min(b.y1),
    }
}

/// The smallest rectangle holding both; an empty one is ignored.
pub(crate) fn union(a: DeviceRect, b: DeviceRect) -> DeviceRect {
    if a.is_empty() {
        return b;
    }
    if b.is_empty() {
        return a;
    }
    DeviceRect {
        x0: a.x0.min(b.x0),
        y0: a.y0.min(b.y0),
        x1: a.x1.max(b.x1),
        y1: a.y1.max(b.y1),
    }
}

/// Pixels in `rect`.
pub(crate) fn area(rect: DeviceRect) -> usize {
    pagura_core::convert::u32_to_usize(rect.width())
        .saturating_mul(pagura_core::convert::u32_to_usize(rect.height()))
}
