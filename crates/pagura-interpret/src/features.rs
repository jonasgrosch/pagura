//! Stable tags for content capabilities that are not implemented.
//!
//! This crate owns the `graphics.`, `extgstate.`, `optionalcontent.`,
//! `pattern.`, `shading.`, `transparency.` and `xobject.` areas.
//! Colour-space tags (`colorspace.`) belong to `pagura-color`, and those of
//! appearance generation (`annotation.`, `appearance.`) to
//! `pagura-appearance`. Interpretation raises most of these; the tags of
//! capabilities that only painting can detect (singular strokes, shading
//! transforms, tiling placement, image masks with patterns and transfer
//! compositing) are defined here too, so that each area has one owner.

use pagura_core::Feature;

/// Stroking under a singular CTM cannot recover the pen's user space.
pub const SINGULAR_STROKE: Feature = Feature::new("graphics.singular-stroke");

/// Black-point compensation requested by `UseBlackPtComp /ON` (Table 58).
pub const EXTGSTATE_BLACK_POINT_COMPENSATION: Feature = Feature::new("extgstate.UseBlackPtComp");

/// Usage-dependent optional content (8.11.4.4).
pub const OPTIONALCONTENT_USAGE: Feature = Feature::new("optionalcontent.usage");

/// XObject subtypes other than Image and Form, such as PostScript (8.8).
pub const XOBJECT_SUBTYPE: Feature = Feature::new("xobject.subtype");

/// JPEG 2000 colour described only inside the codestream: embedded ICC
/// profiles and enumerated spaces other than sRGB and greyscale (8.9.5.3).
pub const XOBJECT_JPX_COLOR: Feature = Feature::new("xobject.jpx-color");

/// Blend modes outside Tables 134 and 135.
pub const TRANSPARENCY_BM: Feature = Feature::new("transparency.BM");

/// Group subtypes other than Transparency (8.10.3).
pub const TRANSPARENCY_GROUP: Feature = Feature::new("transparency.group");

/// Group blending spaces that need an unsupported colour transform (11.7.2).
pub const TRANSPARENCY_GROUP_COLORSPACE: Feature = Feature::new("transparency.group-colorspace");

/// Transfer-function region selection for transparent artwork.
pub const TRANSPARENCY_TRANSFER: Feature = Feature::new("transparency.transfer");

/// Singular shading coordinate systems.
pub const SHADING_TRANSFORM: Feature = Feature::new("shading.transform");

/// Repeated coloured and uncoloured pattern cells the device lattice cannot place.
pub const PATTERN_TILING: Feature = Feature::new("pattern.tiling");

/// Temporary graphics parameters in a shading pattern.
pub const PATTERN_STATE: Feature = Feature::new("pattern.ExtGState");

/// An image mask painted with a pattern (8.9.6.2).
pub const PATTERN_IMAGE_MASK: Feature = Feature::new("pattern.image-mask");

/// Every tag above.
pub const ALL: &[Feature] = &[
    SINGULAR_STROKE,
    EXTGSTATE_BLACK_POINT_COMPENSATION,
    OPTIONALCONTENT_USAGE,
    XOBJECT_SUBTYPE,
    XOBJECT_JPX_COLOR,
    TRANSPARENCY_BM,
    TRANSPARENCY_GROUP,
    TRANSPARENCY_GROUP_COLORSPACE,
    TRANSPARENCY_TRANSFER,
    SHADING_TRANSFORM,
    PATTERN_TILING,
    PATTERN_STATE,
    PATTERN_IMAGE_MASK,
];
