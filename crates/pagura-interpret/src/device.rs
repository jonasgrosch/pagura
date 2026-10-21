//! The device interface (ADR 0034): one interpreter emits a page's marks,
//! groups, masks, patterns and text into any [`Device`].

use crate::model::{
    Clip, Composite, GroupAttributes, Image, MaskId, MaskKind, Paint, PatternId, Pen, Shading,
    TilingColors, ViewFit,
};
use pagura_color::{Components, Function};
use pagura_core::{Name, ObjectId, Position, Rect, Result};
use pagura_document::Rotation;
use pagura_raster::{FillRule, Path, Point, Transform};
use std::sync::Arc;

/// What a device consumes, declared before interpretation starts. The
/// interpreter does no work a device does not need.
///
/// A plain record of independent flags: devices build theirs from a
/// constant with functional record update, such as
/// `Needs { structure: true, ..Needs::SCENE }`, so it is not
/// `#[non_exhaustive]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent capabilities a device declares"
)]
pub struct Needs {
    /// Colours, patterns, shadings, soft masks, groups and transfer
    /// functions are resolved and emitted. Without it, marks carry no
    /// paint and no colour space, shading, pattern or soft mask is read.
    pub paint: bool,
    /// Image marks are emitted with their sources. Without it, images are
    /// not read beyond what text extraction's object occurrences need.
    pub images: bool,
    /// Glyph runs carry outlines (font programs are loaded), and Type 3
    /// glyph procedures are interpreted.
    pub outlines: bool,
    /// Glyph runs carry Unicode text and extraction metrics.
    pub text: bool,
    /// Marked content and object occurrences are emitted, and marks hidden
    /// by optional content are emitted flagged rather than dropped.
    pub structure: bool,
}

impl Needs {
    /// Nothing: the content is only checked.
    pub const NONE: Self = Self {
        paint: false,
        images: false,
        outlines: false,
        text: false,
        structure: false,
    };

    /// Everything.
    pub const ALL: Self = Self {
        paint: true,
        images: true,
        outlines: true,
        text: true,
        structure: true,
    };

    /// What recording a scene for rendering needs.
    pub const SCENE: Self = Self {
        paint: true,
        images: true,
        outlines: true,
        text: false,
        structure: false,
    };

    /// What text extraction needs.
    pub const TEXT: Self = Self {
        paint: false,
        images: false,
        outlines: false,
        text: true,
        structure: true,
    };
}

/// The page a device receives (7.7.3.3, 14.11.2).
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct PageInfo {
    /// The page object.
    pub object: ObjectId,
    /// The crop box, intersected with the media box, in default user space.
    pub crop: Rect,
    /// `Rotate`.
    pub rotation: Rotation,
    /// `UserUnit`.
    pub user_unit: f64,
    /// The page group's attributes, if the page has a `Group`.
    pub group: Option<GroupAttributes>,
    /// Whether the document declares a PDF/X print intent, so overprint is
    /// simulated by default.
    pub print_intent: bool,
}

/// The state every mark carries.
#[derive(Debug, Clone, Copy)]
pub struct MarkState<'a> {
    /// The clipping path chain; `None` clips nothing.
    pub clip: Option<&'a Arc<Clip>>,
    /// The compositing state.
    pub composite: &'a Arc<Composite>,
    /// The operator that painted the mark.
    pub position: Position,
    /// Whether optional content hides the mark (emitted only to devices
    /// that need structure).
    pub hidden: bool,
}

/// A fill (8.5.3).
#[derive(Debug, Clone, Copy)]
pub struct FillMark<'a> {
    /// The path in default user space.
    pub path: &'a Arc<Path>,
    /// The fill rule.
    pub rule: FillRule,
    /// The paint; `None` when the device does not need paint.
    pub paint: Option<&'a Arc<Paint>>,
    /// The state.
    pub state: MarkState<'a>,
}

/// A stroke (8.5.3.2).
#[derive(Debug, Clone, Copy)]
pub struct StrokeMark<'a> {
    /// The path in default user space.
    pub path: &'a Arc<Path>,
    /// The pen.
    pub pen: &'a Arc<Pen>,
    /// The paint; `None` when the device does not need paint.
    pub paint: Option<&'a Arc<Paint>>,
    /// The state.
    pub state: MarkState<'a>,
}

/// A font as text runs identify it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct FontInfo {
    /// A key that identifies the font program within the document, stable
    /// across pages and recordings; it keys glyph caches.
    pub key: FontKey,
    /// The font dictionary, when it is indirect.
    pub object: Option<ObjectId>,
    /// `BaseFont`.
    pub base_name: Option<Name>,
    /// Vertical writing (`WMode 1`).
    pub vertical: bool,
    /// `Subtype`.
    pub kind: pagura_text::FontKind,
}

/// The identity of a font program within one document: never an address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FontKey(pub u64);

/// The identity of one glyph outline: font and glyph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OutlineKey {
    /// The font.
    pub font: FontKey,
    /// The glyph index or code the outline was read for.
    pub glyph: u32,
}

/// One shown glyph.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Glyph {
    /// The character code.
    pub code: u32,
    /// The glyph's outline identity.
    pub outline_key: OutlineKey,
    /// Glyph space (after `FontMatrix`) to default user space: the text
    /// rendering matrix and the CTM.
    pub transform: Transform,
    /// The outline in glyph space, when the device needs outlines and the
    /// font has outlines (not for Type 3 fonts, whose glyph procedures are
    /// interpreted into ordinary marks).
    pub outline: Option<Arc<Path>>,
    /// Text for extraction, when the device needs text.
    pub text: Option<Box<pagura_extract::PositionedGlyph>>,
    /// The string operand that showed it.
    pub position: Position,
}

/// Text rendering of a run (9.3.6): what the glyphs are painted with.
#[derive(Debug, Clone, Copy)]
pub struct GlyphPaint<'a> {
    /// The fill paint, for modes that fill.
    pub fill: Option<&'a Arc<Paint>>,
    /// The fill state.
    pub fill_state: Option<MarkState<'a>>,
    /// The stroke paint and pen, for modes that stroke.
    pub stroke: Option<(&'a Arc<Paint>, &'a Arc<Pen>)>,
    /// The stroke state.
    pub stroke_state: Option<MarkState<'a>>,
}

/// Glyphs shown by one string operand in one font and state.
#[derive(Debug, Clone, Copy)]
pub struct GlyphRun<'a> {
    /// The font.
    pub font: &'a FontInfo,
    /// The glyphs in show order.
    pub glyphs: &'a [Glyph],
    /// The rendering mode.
    pub mode: pagura_extract::TextRenderingMode,
    /// How the glyphs are painted; empty for invisible and clip-only text.
    pub paint: GlyphPaint<'a>,
    /// Whether optional content hides the run.
    pub hidden: bool,
}

/// An image mark (8.9.5) or a stencil mask painted with the fill paint.
#[derive(Debug, Clone, Copy)]
pub struct ImageMark<'a> {
    /// The image.
    pub image: &'a Arc<Image>,
    /// Unit square to default user space: the CTM.
    pub transform: Transform,
    /// The fill paint of a stencil mask.
    pub stencil: Option<&'a Arc<Paint>>,
    /// The state.
    pub state: MarkState<'a>,
}

/// A shading painted with `sh` (8.7.4.3).
#[derive(Debug, Clone, Copy)]
pub struct ShadingMark<'a> {
    /// The shading.
    pub shading: &'a Arc<Shading>,
    /// Shading space to default user space: the CTM.
    pub transform: Transform,
    /// The state.
    pub state: MarkState<'a>,
}

/// A transparency group (11.6.6) or an annotation's fixed view.
#[derive(Debug, Clone, Copy)]
pub struct GroupInfo<'a> {
    /// Its attributes.
    pub attributes: GroupAttributes,
    /// Its bounding box in default user space (`BBox` under the form matrix
    /// and CTM): a bound for its buffer only. The `BBox` clips the group's
    /// marks through their clip chains, which include it.
    pub bbox: Option<&'a Arc<Path>>,
    /// A fixed view, for annotations with `NoZoom` or `NoRotate`.
    pub view: Option<ViewFit>,
    /// The state the group's result is composited with.
    pub state: MarkState<'a>,
}

/// A soft mask's definition (11.6.5.2); its group's content follows until
/// `end_soft_mask`.
#[derive(Debug, Clone, Copy)]
pub struct SoftMaskInfo<'a> {
    /// The mask's number, used by later composite states.
    pub id: MaskId,
    /// Alpha or luminosity.
    pub kind: MaskKind,
    /// The mask group's attributes.
    pub group: GroupAttributes,
    /// The mask group's bounding box in default user space: a bound for
    /// its buffer only; the `BBox` clips the group's marks through their
    /// clip chains.
    pub bbox: Option<&'a Arc<Path>>,
    /// `BC`, in the group's blending space.
    pub backdrop: Option<&'a Components>,
    /// `TR`.
    pub transfer: Option<&'a Arc<Function>>,
    /// The `gs` operator that set it.
    pub position: Position,
}

/// A tiling pattern's definition (8.7.3); its cell's content (in pattern
/// space) follows until `end_tiling`.
#[derive(Debug, Clone, Copy)]
pub struct TilingInfo {
    /// The pattern's number, used by later paints.
    pub id: PatternId,
    /// `BBox` in pattern space.
    pub bbox: Rect,
    /// `XStep` and `YStep`.
    pub step: [f64; 2],
    /// Pattern space to default user space.
    pub matrix: Transform,
    /// `PaintType`.
    pub colors: TilingColors,
    /// The pattern's object, for caches.
    pub object: Option<ObjectId>,
}

/// The marked-content properties text extraction reads (14.6.2, 14.7.5.2,
/// 14.9.3–14.9.4), parsed for devices that need text. Malformed values are
/// reported repairs and read as absent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct MarkedText {
    /// `MCID`.
    pub mcid: Option<i64>,
    /// `ActualText`, decoded.
    pub actual_text: Option<String>,
    /// `Alt`, decoded.
    pub alt: Option<String>,
}

/// Marked content, object occurrences and annotation boundaries, for
/// devices that need structure.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub enum Marked<'a> {
    /// `BMC` or `BDC` (14.6).
    Begin {
        /// The tag.
        tag: &'a Name,
        /// The properties dictionary, resolved; `None` for `BMC` and for
        /// properties that could not be read.
        properties: Option<&'a pagura_core::Dictionary>,
        /// Whether optional content hides the sequence, this one's
        /// membership included.
        hidden: bool,
        /// The properties text extraction reads, for devices that need text
        /// and in content whose text belongs to the page (not annotation
        /// appearances, patterns, soft masks or Type 3 glyphs).
        text: Option<&'a MarkedText>,
    },
    /// `EMC`.
    End,
    /// An XObject or annotation occurrence starts (for structure `OBJR`).
    /// The text shown until the matching [`Marked::ObjectEnd`] belongs to
    /// it; images and annotations end at once.
    ObjectBegin {
        /// The XObject or annotation.
        object: ObjectId,
        /// Whether optional content hides the occurrence.
        hidden: bool,
    },
    /// The innermost object occurrence ends.
    ObjectEnd,
}

/// A sink for interpreted content. Every method has an empty default, so a
/// device implements what it consumes. Calls are balanced: every `begin_*`
/// has its `end_*`, and definitions come before their first use.
///
/// The interpreter is generic over the device, so pixel devices are
/// monomorphized; `dyn Device` serves cold sinks.
#[expect(
    unused_variables,
    reason = "the default methods ignore their arguments; implementors name them"
)]
pub trait Device {
    /// What the device needs.
    fn needs(&self) -> Needs;

    /// The page starts.
    ///
    /// # Errors
    ///
    /// A device error stops interpretation; so for every method.
    fn begin_page(&mut self, page: &PageInfo) -> Result<()> {
        Ok(())
    }

    /// The page ends.
    ///
    /// # Errors
    ///
    /// As [`Self::begin_page`].
    fn end_page(&mut self) -> Result<()> {
        Ok(())
    }

    /// A fill.
    ///
    /// # Errors
    ///
    /// As [`Self::begin_page`].
    fn fill(&mut self, mark: &FillMark<'_>) -> Result<()> {
        Ok(())
    }

    /// A stroke.
    ///
    /// # Errors
    ///
    /// As [`Self::begin_page`].
    fn stroke(&mut self, mark: &StrokeMark<'_>) -> Result<()> {
        Ok(())
    }

    /// Glyphs shown by one string operand.
    ///
    /// # Errors
    ///
    /// As [`Self::begin_page`].
    fn glyphs(&mut self, run: &GlyphRun<'_>) -> Result<()> {
        Ok(())
    }

    /// An image or stencil mask.
    ///
    /// # Errors
    ///
    /// As [`Self::begin_page`].
    fn image(&mut self, mark: &ImageMark<'_>) -> Result<()> {
        Ok(())
    }

    /// A shading painted with `sh`.
    ///
    /// # Errors
    ///
    /// As [`Self::begin_page`].
    fn shading(&mut self, mark: &ShadingMark<'_>) -> Result<()> {
        Ok(())
    }

    /// A transparency group starts; its content follows.
    ///
    /// # Errors
    ///
    /// As [`Self::begin_page`].
    fn begin_group(&mut self, group: &GroupInfo<'_>) -> Result<()> {
        Ok(())
    }

    /// The innermost open group ends.
    ///
    /// # Errors
    ///
    /// As [`Self::begin_page`].
    fn end_group(&mut self) -> Result<()> {
        Ok(())
    }

    /// A soft mask's group starts; its content follows. Only devices that
    /// need paint receive soft masks.
    ///
    /// # Errors
    ///
    /// As [`Self::begin_page`].
    fn begin_soft_mask(&mut self, mask: &SoftMaskInfo<'_>) -> Result<()> {
        Ok(())
    }

    /// The soft mask's group ends.
    ///
    /// # Errors
    ///
    /// As [`Self::begin_page`].
    fn end_soft_mask(&mut self) -> Result<()> {
        Ok(())
    }

    /// A tiling pattern's cell starts; its content follows. Only devices
    /// that need paint receive patterns.
    ///
    /// # Errors
    ///
    /// As [`Self::begin_page`].
    fn begin_tiling(&mut self, tiling: &TilingInfo) -> Result<()> {
        Ok(())
    }

    /// The tiling pattern's cell ends.
    ///
    /// # Errors
    ///
    /// As [`Self::begin_page`].
    fn end_tiling(&mut self) -> Result<()> {
        Ok(())
    }

    /// A font is selected (`Tf`, or `Font` in a graphics state parameter
    /// dictionary), in selection order; only devices that need text
    /// receive it.
    ///
    /// # Errors
    ///
    /// As [`Self::begin_page`].
    fn select_font(&mut self, font: &FontInfo) -> Result<()> {
        Ok(())
    }

    /// Marked content and object boundaries; only devices that need
    /// structure receive them.
    ///
    /// # Errors
    ///
    /// As [`Self::begin_page`].
    fn marked(&mut self, event: &Marked<'_>) -> Result<()> {
        Ok(())
    }
}

/// A glyph origin in default user space: where `transform` maps the glyph
/// space origin.
pub fn glyph_origin(glyph: &Glyph) -> Point {
    Point::new(glyph.transform.e, glyph.transform.f)
}
