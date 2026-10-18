//! Stable tags for rendering capabilities that are not implemented.
//!
//! This crate owns the `graphics.`, `annotation.`, `appearance.`,
//! `extgstate.`, `optionalcontent.`, `pattern.`, `shading.`,
//! `transparency.`, `xobject.` and `function.` areas. Every Annex A
//! operator is implemented, so no `operator.` tag is produced.
//! Colour-space tags (`colorspace.`) belong to `pagura-color`.

use pagura_core::Feature;

/// Stroking under a singular CTM cannot recover the pen's user space.
pub const SINGULAR_STROKE: Feature = Feature::new("graphics.singular-stroke");

/// Generating an appearance for an annotation without `AP` outside the
/// supported subtypes and borders (12.5.6).
pub const ANNOTATION_APPEARANCE: Feature = Feature::new("annotation.appearance");

/// Black-point compensation requested by `UseBlackPtComp /ON` (Table 58).
pub const EXTGSTATE_BLACK_POINT_COMPENSATION: Feature = Feature::new("extgstate.UseBlackPtComp");

/// Usage-dependent optional content (8.11.4.4).
pub const OPTIONALCONTENT_USAGE: Feature = Feature::new("optionalcontent.usage");

/// XObject subtypes other than Image and Form, such as PostScript (8.8).
pub const XOBJECT_SUBTYPE: Feature = Feature::new("xobject.subtype");

/// JPEG 2000 colour described only inside the codestream: embedded ICC
/// profiles and enumerated spaces other than sRGB and greyscale (8.9.5.3).
pub const XOBJECT_JPX_COLOR: Feature = Feature::new("xobject.jpx-color");

/// Image opacity carried in the image data (`SMaskInData`, 11.6.5.3).
pub const TRANSPARENCY_SMASK_IN_DATA: Feature = Feature::new("transparency.SMaskInData");

/// Blend modes outside Tables 134 and 135.
pub const TRANSPARENCY_BM: Feature = Feature::new("transparency.BM");

/// Group subtypes other than Transparency (8.10.3).
pub const TRANSPARENCY_GROUP: Feature = Feature::new("transparency.group");

/// Group blending spaces that need an unsupported colour transform (11.7.2).
pub const TRANSPARENCY_GROUP_COLORSPACE: Feature = Feature::new("transparency.group-colorspace");

/// Transfer-function region selection for transparent artwork.
pub const TRANSPARENCY_TRANSFER: Feature = Feature::new("transparency.transfer");

/// Process-colour overprint inside a subtractive blending space.
pub const TRANSPARENCY_OVERPRINT: Feature = Feature::new("transparency.overprint");

/// Patch mesh points whose inverse cannot be resolved numerically.
pub const SHADING_MESH: Feature = Feature::new("shading.mesh");

/// Singular shading coordinate systems.
pub const SHADING_TRANSFORM: Feature = Feature::new("shading.transform");

/// Repeated coloured and uncoloured pattern cells the device lattice cannot place.
pub const PATTERN_TILING: Feature = Feature::new("pattern.tiling");

/// Temporary graphics parameters in a shading pattern.
pub const PATTERN_STATE: Feature = Feature::new("pattern.ExtGState");

/// An image mask painted with a pattern (8.9.6.2).
pub const PATTERN_IMAGE_MASK: Feature = Feature::new("pattern.image-mask");

/// Oblique, reflected or degenerate text transforms and negative font sizes
/// in a generated form appearance (12.7.4.3).
pub const APPEARANCE_TEXT_MATRIX: Feature = Feature::new("appearance.text-matrix");

/// Default appearance operators outside the supported subset (12.7.4.3).
pub const APPEARANCE_DEFAULT_APPEARANCE: Feature = Feature::new("appearance.default-appearance");

/// Field text without a reversible simple horizontal font encoding.
pub const APPEARANCE_FONT_ENCODING: Feature = Feature::new("appearance.font-encoding");

/// Rounded, beveled and inset widget border generation.
pub const APPEARANCE_BORDER_STYLE: Feature = Feature::new("appearance.border-style");

/// Appearance generation for a rotated widget.
pub const APPEARANCE_WIDGET_ROTATION: Feature = Feature::new("appearance.widget-rotation");

/// Inline images, multiple variable text sections or crossed content scopes
/// in an appearance being updated.
pub const APPEARANCE_UPDATE: Feature = Feature::new("appearance.update");

/// Every tag above.
pub const ALL: &[Feature] = &[
    SINGULAR_STROKE,
    ANNOTATION_APPEARANCE,
    EXTGSTATE_BLACK_POINT_COMPENSATION,
    OPTIONALCONTENT_USAGE,
    XOBJECT_SUBTYPE,
    XOBJECT_JPX_COLOR,
    TRANSPARENCY_SMASK_IN_DATA,
    TRANSPARENCY_BM,
    TRANSPARENCY_GROUP,
    TRANSPARENCY_GROUP_COLORSPACE,
    TRANSPARENCY_TRANSFER,
    TRANSPARENCY_OVERPRINT,
    SHADING_MESH,
    SHADING_TRANSFORM,
    PATTERN_TILING,
    PATTERN_STATE,
    PATTERN_IMAGE_MASK,
    APPEARANCE_TEXT_MATRIX,
    APPEARANCE_DEFAULT_APPEARANCE,
    APPEARANCE_FONT_ENCODING,
    APPEARANCE_BORDER_STYLE,
    APPEARANCE_WIDGET_ROTATION,
    APPEARANCE_UPDATE,
];
