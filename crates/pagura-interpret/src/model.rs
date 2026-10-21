//! The values the interpreter hands to devices and the scene records: paints,
//! pens, clips, composite states, shadings, images and group attributes, all
//! in PDF terms and free of device decisions.
//!
//! Geometry is in the page's default user space (the CTM is applied when a
//! path is built), except tiling-cell content, which is in pattern space.

use pagura_color::{ColorSpace, Components, Conversion, Function, Process};
use pagura_core::{ObjectId, Rect};
use pagura_raster::{Adjustment, FillRule, Path, Point, Stroke, Transform};
use std::sync::Arc;

/// A soft mask defined by `begin_soft_mask`, numbered densely in definition
/// order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MaskId(pub u32);

/// A tiling pattern defined by `begin_tiling`, numbered densely in
/// definition order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PatternId(pub u32);

/// The separable and non-separable blend modes (11.3.5, Tables 134–135).
/// The standard closes the set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum BlendMode {
    /// Normal (also `Compatible`).
    #[default]
    Normal,
    /// Multiply.
    Multiply,
    /// Screen.
    Screen,
    /// Overlay.
    Overlay,
    /// Darken.
    Darken,
    /// Lighten.
    Lighten,
    /// ColorDodge.
    ColorDodge,
    /// ColorBurn.
    ColorBurn,
    /// HardLight.
    HardLight,
    /// SoftLight.
    SoftLight,
    /// Difference.
    Difference,
    /// Exclusion.
    Exclusion,
    /// Hue.
    Hue,
    /// Saturation.
    Saturation,
    /// Color.
    Color,
    /// Luminosity.
    Luminosity,
}

impl BlendMode {
    /// Whether the mode is separable (Table 134).
    pub fn is_separable(self) -> bool {
        !matches!(
            self,
            Self::Hue | Self::Saturation | Self::Color | Self::Luminosity
        )
    }

    /// Whether the mode amplifies differences of its inputs, so that a buffer
    /// feeding it needs f64 storage to stay within one level (ADR 0004):
    /// ColorDodge, ColorBurn, Overlay, HardLight, SoftLight and the
    /// non-separable modes.
    pub fn amplifies(self) -> bool {
        matches!(
            self,
            Self::ColorDodge | Self::ColorBurn | Self::Overlay | Self::HardLight | Self::SoftLight
        ) || !self.is_separable()
    }
}

/// Transfer functions (10.5): one for every component, or one per
/// component in the order of the blending space's components.
#[derive(Debug, Clone)]
pub enum Transfer {
    /// The same function for every component.
    All(Arc<Function>),
    /// One function per component; `None` is the identity.
    PerComponent([Option<Arc<Function>>; 4]),
}

/// Overprint parameters of a mark (8.6.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Overprint {
    /// `OP` for strokes, `op` for fills and other marks.
    pub enabled: bool,
    /// `OPM 1`: zero components of DeviceCMYK colours leave the backdrop.
    pub nonzero: bool,
}

/// The compositing state of a mark or group (11.3, 11.6), interned: marks
/// with the same state share one value.
///
/// A plain record built field by field (functional record update from
/// [`Default`] included), so it is not `#[non_exhaustive]`.
#[derive(Debug, Clone)]
pub struct Composite {
    /// Constant alpha, `CA` for strokes and `ca` for other marks.
    pub alpha: f64,
    /// Blend mode.
    pub blend: BlendMode,
    /// The soft mask, if any.
    pub soft_mask: Option<MaskId>,
    /// `AIS`: alpha sources are shape.
    pub alpha_is_shape: bool,
    /// Overprint.
    pub overprint: Overprint,
    /// Transfer functions, if any.
    pub transfer: Option<Arc<Transfer>>,
    /// Rendering intent and black generation and undercolour removal.
    pub conversion: Conversion,
}

impl Default for Composite {
    fn default() -> Self {
        Self {
            alpha: 1.0,
            blend: BlendMode::Normal,
            soft_mask: None,
            alpha_is_shape: false,
            overprint: Overprint::default(),
            transfer: None,
            conversion: Conversion::default(),
        }
    }
}

impl Composite {
    /// Whether the state composites as plain opaque Normal source-over: alpha
    /// 1, Normal blend, no soft mask and no transfer.
    pub fn is_plain(&self) -> bool {
        self.alpha == 1.0
            && self.blend == BlendMode::Normal
            && self.soft_mask.is_none()
            && self.transfer.is_none()
    }
}

/// The states of a fill and stroke painted by one operator (`B`, `b`, text
/// rendering modes 2 and 6), which are one object for transparency
/// (11.7.4.4): a knockout group, or, when both overprint with equal alpha,
/// a non-knockout group that carries the shared alpha, blend mode and soft
/// mask while its members paint opaquely.
#[derive(Debug, Clone)]
pub(crate) struct FillStroke {
    /// The group's state.
    pub(crate) group: Arc<Composite>,
    /// Whether the group knocks out.
    pub(crate) knockout: bool,
    /// The fill's state.
    pub(crate) fill: Arc<Composite>,
    /// The stroke's state.
    pub(crate) stroke: Arc<Composite>,
}

impl FillStroke {
    /// The states for a fill in `fill` and a stroke in `stroke`; `opaque`
    /// is the plain state of a knockout group.
    pub(crate) fn new(fill: &Arc<Composite>, stroke: &Arc<Composite>, opaque: &Arc<Composite>) -> Self {
        let overprint =
            fill.overprint.enabled && stroke.overprint.enabled && fill.alpha == stroke.alpha;
        if overprint {
            let member = |composite: &Composite| {
                Arc::new(Composite {
                    alpha: 1.0,
                    blend: BlendMode::Normal,
                    soft_mask: None,
                    ..composite.clone()
                })
            };
            Self {
                group: Arc::clone(fill),
                knockout: false,
                fill: member(fill),
                stroke: member(stroke),
            }
        } else {
            Self {
                group: Arc::clone(opaque),
                knockout: true,
                fill: Arc::clone(fill),
                stroke: Arc::clone(stroke),
            }
        }
    }
}

/// One link of a clipping path chain (8.5.4): the intersection of `path`
/// with every ancestor. Chains are persistent: a `q`/`Q` pair restores a
/// shorter chain without copying.
#[derive(Debug)]
pub struct Clip {
    parent: Option<Arc<Clip>>,
    path: Arc<Path>,
    rule: FillRule,
    depth: u32,
}

impl Clip {
    /// A clip that intersects `path` (default user space) with `parent`.
    pub fn new(parent: Option<Arc<Self>>, path: Arc<Path>, rule: FillRule) -> Self {
        let depth = parent.as_ref().map_or(1, |p| p.depth.saturating_add(1));
        Self {
            parent,
            path,
            rule,
            depth,
        }
    }

    /// The enclosing clip.
    pub fn parent(&self) -> Option<&Arc<Self>> {
        self.parent.as_ref()
    }

    /// The clipping path in default user space.
    pub fn path(&self) -> &Arc<Path> {
        &self.path
    }

    /// The fill rule.
    pub fn rule(&self) -> FillRule {
        self.rule
    }

    /// Paths in the chain, this one included.
    pub fn depth(&self) -> u32 {
        self.depth
    }
}

/// A colour in its source space.
#[derive(Debug, Clone)]
pub struct Color {
    /// The colour space.
    pub space: Arc<ColorSpace>,
    /// The components.
    pub components: Components,
}

/// A shading pattern as paint (8.7.4.2).
#[derive(Debug, Clone)]
pub struct ShadingPaint {
    /// The shading.
    pub shading: Arc<Shading>,
    /// Shading space to default user space: the pattern matrix.
    pub matrix: Transform,
}

/// A tiling pattern as paint (8.7.3).
#[derive(Debug, Clone)]
pub struct TilingPaint {
    /// The pattern, defined earlier by `begin_tiling`.
    pub pattern: PatternId,
    /// The colour of an uncoloured pattern.
    pub color: Option<Color>,
}

/// What a mark is painted with.
#[derive(Debug, Clone)]
pub enum Paint {
    /// A colour.
    Solid(Color),
    /// A shading pattern.
    Shading(ShadingPaint),
    /// A tiling pattern.
    Tiling(TilingPaint),
}

/// The pen of a stroke.
#[derive(Debug, Clone)]
pub struct Pen {
    /// Width, caps, joins, miter limit and dashes, in pen space.
    pub stroke: Stroke,
    /// Pen space to default user space: the CTM when the stroke was painted.
    pub transform: Transform,
    /// Stroke adjustment (`SA`).
    pub adjustment: Adjustment,
}

/// A shading after parsing and validation (8.7.4.5).
///
/// A plain record the interpreter fills field by field, so it is not
/// `#[non_exhaustive]`.
#[derive(Debug, Clone)]
pub struct Shading {
    /// The shading's object, when it is indirect; identity for caches.
    pub object: Option<ObjectId>,
    /// The colour space of the shading's colours.
    pub space: Arc<ColorSpace>,
    /// `Background`, used only by shading patterns.
    pub background: Option<Components>,
    /// `BBox` in shading space.
    pub bbox: Option<Rect>,
    /// `AntiAlias`.
    pub antialias: bool,
    /// The geometry.
    pub kind: ShadingKind,
    /// `Function`, when present.
    pub function: Option<ShadingFunction>,
}

/// A shading's colour function: one with `n` outputs, or one per colour
/// component.
#[derive(Debug, Clone)]
pub enum ShadingFunction {
    /// One function for every component.
    Single(Arc<Function>),
    /// One single-output function per component.
    PerComponent(Arc<[Arc<Function>]>),
}

/// The geometry of a shading (8.7.4.5.2–8.7.4.5.8).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ShadingKind {
    /// Type 1, function-based.
    Function {
        /// `Domain` as `[x0, x1, y0, y1]`.
        domain: [f64; 4],
        /// `Matrix`: function domain space to shading space.
        matrix: Transform,
    },
    /// Type 2, axial.
    Axial {
        /// Start of the axis.
        start: Point,
        /// End of the axis.
        end: Point,
        /// `Domain` `[t0, t1]`.
        domain: [f64; 2],
        /// `Extend` before the start and after the end.
        extend: [bool; 2],
    },
    /// Type 3, radial.
    Radial {
        /// Centre of the start circle.
        start: Point,
        /// Radius of the start circle.
        start_radius: f64,
        /// Centre of the end circle.
        end: Point,
        /// Radius of the end circle.
        end_radius: f64,
        /// `Domain` `[t0, t1]`.
        domain: [f64; 2],
        /// `Extend`.
        extend: [bool; 2],
    },
    /// Types 4 and 5, triangle meshes.
    Triangles(Arc<[[MeshVertex; 3]]>),
    /// Types 6 and 7, Coons and tensor-product patches, as tensor patches.
    Patches(Arc<[Patch]>),
}

/// A mesh vertex: its point in shading space and its colour, or the single
/// parametric value `t` of a shading with a function.
#[derive(Debug, Clone)]
pub struct MeshVertex {
    /// The point.
    pub point: Point,
    /// Colour components, or `[t]`.
    pub color: Components,
}

/// A tensor-product patch (8.7.4.5.8). `points[4 * i + j]` is the control
/// point `p_ij` of 8.7.4.5.8, where `i` is the index along `u` and `j` the
/// index along `v`; Coons patches (type 6) carry the four interior points
/// the Coons surface implies. `colors` are the colours of the corners
/// `p00`, `p03`, `p33` and `p30`, in that order.
#[derive(Debug, Clone)]
pub struct Patch {
    /// Control points.
    pub points: [Point; 16],
    /// Corner colours, or `[t]` values.
    pub colors: [Components; 4],
}

/// An image ready for painting: its undecoded source and how to interpret it.
///
/// A plain record the interpreter fills field by field, so it is not
/// `#[non_exhaustive]`.
#[derive(Debug, Clone)]
pub struct Image {
    /// The samples, decoded on request.
    pub source: Arc<pagura_image::ImageSource>,
    /// The colour space; `None` for a stencil mask.
    pub space: Option<Arc<ColorSpace>>,
    /// `Decode`, one interval per component.
    pub decode: Arc<[[f64; 2]]>,
    /// `Interpolate`.
    pub interpolate: bool,
    /// `Intent`, if the image overrides the graphics state's.
    pub intent: Option<pagura_color::Intent>,
    /// Masking (8.9.6).
    pub mask: ImageMask,
}

/// The masking of an image (8.9.6).
#[derive(Debug, Clone, Default)]
pub enum ImageMask {
    /// None.
    #[default]
    None,
    /// Colour-key masking: inclusive ranges of raw sample values per
    /// component.
    ColorKey(Arc<[[u16; 2]]>),
    /// An explicit stencil mask (`Mask` stream).
    Stencil(Arc<Image>),
    /// A soft-mask image (`SMask`), with its `Matte` if any.
    Soft {
        /// The mask image, one gray component.
        mask: Arc<Image>,
        /// Colour the image was premultiplied with.
        matte: Option<Components>,
    },
    /// Alpha carried in the image data (`SMaskInData`).
    Embedded,
}

/// Transparency group attributes (11.6.6, Table 145).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GroupAttributes {
    /// `I`.
    pub isolated: bool,
    /// `K`.
    pub knockout: bool,
    /// The blending space `CS`, reduced to its device family; `None` inherits.
    pub space: Option<Process>,
}

/// Why a group exists. The document declares some; the interpreter adds
/// the others for one compositing rule each, never isolated and with a plain
/// state (opaque, Normal, no soft mask) except where noted, and without a
/// clip of their own: their marks carry their clips. Such a group whose
/// members all composite opaquely with Normal blending paints as its
/// members painted in its place (11.4.6: an opaque Normal member knocks out
/// exactly as it composites source-over).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum GroupCause {
    /// Declared by the document: the page group, a form's `Group`, a soft
    /// mask's group or a tiling cell.
    #[default]
    Document,
    /// An annotation appearance without a group of its own, drawn inside
    /// one (12.5.5), or the fixed view of an annotation (12.5.3), which
    /// keeps its [`ViewFit`].
    Annotation,
    /// The glyphs of one text object under text knockout (`TK`, 9.3.8): a
    /// knockout group.
    TextKnockout,
    /// A fill and stroke painted by one operator (`B`, `b`, text rendering
    /// modes 2 and 6) as one object (11.7.4.4): a knockout group, or, when
    /// both overprint with equal alpha, a group carrying their shared
    /// alpha, blend mode and soft mask.
    FillStroke,
    /// The marks of one Type 3 glyph procedure.
    Type3Glyph,
}

impl GroupCause {
    /// Whether the interpreter added the group for a compositing rule
    /// rather than the document declaring it.
    pub fn is_implicit(self) -> bool {
        self != Self::Document
    }
}

/// A fixed view of an annotation (12.5.3, `NoZoom` and `NoRotate`): the
/// group keeps its size or orientation around `anchor` on the device.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ViewFit {
    /// The anchor in default user space: the annotation's upper-left corner.
    pub anchor: Point,
    /// The group does not scale with the device.
    pub no_zoom: bool,
    /// The group does not rotate with the page.
    pub no_rotate: bool,
}

/// How a soft mask's group becomes mask values (11.6.5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum MaskKind {
    /// The group's alpha.
    #[default]
    Alpha,
    /// The luminosity of the group's colour over its backdrop.
    Luminosity,
}

/// Whether a tiling pattern's cell paints its own colours (8.7.3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TilingColors {
    /// `PaintType 1`.
    #[default]
    Colored,
    /// `PaintType 2`: the cell is a stencil painted with the use's colour.
    Uncolored,
}
