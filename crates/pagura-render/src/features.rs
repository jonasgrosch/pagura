//! Stable rendering capability tags.

use pagura_core::Feature;

/// Stroking under a singular CTM cannot recover the pen's user space.
pub const SINGULAR_STROKE: Feature = Feature::new("graphics.singular-stroke");

/// Unsupported capability `annotation.appearance`.
pub const ANNOTATION_APPEARANCE: Feature = Feature::new("annotation.appearance");

/// Unsupported capability `colorspace.CalGray`.
pub const COLORSPACE_CALGRAY: Feature = Feature::new("colorspace.CalGray");

/// Unsupported capability `colorspace.CalRGB`.
pub const COLORSPACE_CALRGB: Feature = Feature::new("colorspace.CalRGB");

/// Unsupported capability `colorspace.DeviceN`.
pub const COLORSPACE_DEVICEN: Feature = Feature::new("colorspace.DeviceN");

/// Unsupported capability `colorspace.Lab`.
pub const COLORSPACE_LAB: Feature = Feature::new("colorspace.Lab");

/// Unsupported capability `colorspace.Separation`.
pub const COLORSPACE_SEPARATION: Feature = Feature::new("colorspace.Separation");

/// Unsupported capability `colorspace.unknown`.
pub const COLORSPACE_UNKNOWN: Feature = Feature::new("colorspace.unknown");

/// Unsupported capability `extgstate.BG`.
pub const EXTGSTATE_BG: Feature = Feature::new("extgstate.BG");

/// Unsupported capability `extgstate.Font`.
pub const EXTGSTATE_FONT: Feature = Feature::new("extgstate.Font");

/// Unsupported capability `extgstate.HT`.
pub const EXTGSTATE_HT: Feature = Feature::new("extgstate.HT");

/// Unsupported capability `extgstate.SM`.
pub const EXTGSTATE_SM: Feature = Feature::new("extgstate.SM");

/// Unsupported capability `extgstate.TK`.
pub const EXTGSTATE_TK: Feature = Feature::new("extgstate.TK");

/// Unsupported capability `extgstate.TR`.
pub const EXTGSTATE_TR: Feature = Feature::new("extgstate.TR");

/// Unsupported capability `extgstate.UCR`.
pub const EXTGSTATE_UCR: Feature = Feature::new("extgstate.UCR");

/// Unsupported capability `extgstate.unknown`.
pub const EXTGSTATE_UNKNOWN: Feature = Feature::new("extgstate.unknown");

/// Unsupported capability `graphics.stroke-adjustment`.
pub const GRAPHICS_STROKE_ADJUSTMENT: Feature = Feature::new("graphics.stroke-adjustment");

/// Unsupported capability `image.inline`.
pub const IMAGE_INLINE: Feature = Feature::new("image.inline");

/// Unsupported capability `image.xobject`.
pub const IMAGE_XOBJECT: Feature = Feature::new("image.xobject");

/// Unsupported capability `optionalcontent.usage`.
pub const OPTIONALCONTENT_USAGE: Feature = Feature::new("optionalcontent.usage");

/// Unsupported capability `pattern.colorspace`.
pub const PATTERN_COLORSPACE: Feature = Feature::new("pattern.colorspace");

/// Unsupported capability `shading.paint`.
pub const SHADING_PAINT: Feature = Feature::new("shading.paint");

/// Unsupported capability `transparency.BM`.
pub const TRANSPARENCY_BM: Feature = Feature::new("transparency.BM");

/// Unsupported capability `transparency.CA`.
pub const TRANSPARENCY_CA_STROKING: Feature = Feature::new("transparency.CA");

/// Unsupported capability `transparency.SMask`.
pub const TRANSPARENCY_SMASK: Feature = Feature::new("transparency.SMask");

/// Unsupported capability `transparency.ca`.
pub const TRANSPARENCY_CA: Feature = Feature::new("transparency.ca");

/// Unsupported capability `transparency.group`.
pub const TRANSPARENCY_GROUP: Feature = Feature::new("transparency.group");

/// Unsupported capability `transparency.group-colorspace`.
pub const TRANSPARENCY_GROUP_COLORSPACE: Feature = Feature::new("transparency.group-colorspace");

/// Unsupported capability `transparency.knockout`.
pub const TRANSPARENCY_KNOCKOUT: Feature = Feature::new("transparency.knockout");

/// Unsupported capability `xobject.subtype`.
pub const XOBJECT_SUBTYPE: Feature = Feature::new("xobject.subtype");

macro_rules! operator_tags {
    ($($name:literal),* $(,)?) => {
        /// All recognized Annex A operator tags, in PDF spelling.
        pub const OPERATORS: &[Feature] = &[$(Feature::new(concat!("operator.", $name))),*];
        pub(crate) fn operator_tag(operator: &[u8]) -> Feature {
            match std::str::from_utf8(operator) {
                $(Ok($name) => { const TAG: Feature = Feature::new(concat!("operator.", $name)); TAG }),*
                _ => OPERATOR_INVALID,
            }
        }
    }
}
/// A defensive tag for an operator outside the recognized table.
pub const OPERATOR_INVALID: Feature = Feature::new("operator.invalid");
operator_tags!(
    "b", "B", "b*", "B*", "BDC", "BI", "BMC", "BT", "BX", "c", "cm", "CS", "cs", "d", "d0", "d1",
    "Do", "DP", "EI", "EMC", "ET", "EX", "f", "F", "f*", "G", "g", "gs", "h", "i", "ID", "j", "J",
    "K", "k", "l", "m", "M", "MP", "n", "q", "Q", "re", "RG", "rg", "ri", "s", "S", "SC", "SCN",
    "sc", "scn", "sh", "T*", "Tc", "Td", "TD", "Tf", "Tj", "TJ", "TL", "Tm", "Tr", "Ts", "Tw",
    "Tz", "v", "w", "W", "W*", "y", "'", "\""
);

/// Unsupported ICC range interpretation (retained feature-tag API).
pub const COLORSPACE_ICC_RANGE: Feature = Feature::new("colorspace.ICCRange");

/// Nonzero calibrated black points require black-point compensation.
pub const COLORSPACE_BLACKPOINT: Feature = Feature::new("colorspace.BlackPoint");

/// PostScript-calculator functions.
pub const FUNCTION_CALCULATOR: Feature = Feature::new("function.Type4");
/// Separation names None and All have special device semantics.
pub const COLORSPACE_SPECIAL_INK: Feature = Feature::new("colorspace.special-ink");

/// Annotation fitting that preserves display size or orientation.
pub const ANNOTATION_FLAGS: Feature = Feature::new("annotation.flags");

/// Transfer-function region selection for transparent artwork.
pub const TRANSPARENCY_TRANSFER: Feature = Feature::new("transparency.transfer");
/// Process-colour overprint inside a subtractive blending space.
pub const TRANSPARENCY_OVERPRINT: Feature = Feature::new("transparency.overprint");

/// Triangle and patch mesh shading sources.
pub const SHADING_MESH: Feature = Feature::new("shading.mesh");
/// Singular shading coordinate systems.
pub const SHADING_TRANSFORM: Feature = Feature::new("shading.transform");
/// Zero-width or zero-height shading boxes requiring scan-conversion adjustment.
pub const SHADING_BOUNDS: Feature = Feature::new("shading.degenerate-bounds");
/// Repeated coloured and uncoloured pattern cells.
pub const PATTERN_TILING: Feature = Feature::new("pattern.tiling");
/// Temporary graphics parameters in a shading pattern.
pub const PATTERN_STATE: Feature = Feature::new("pattern.ExtGState");

/// ICC profile processing outside the supported forward matrix/TRC and LUT subset.
pub const COLORSPACE_ICC: Feature = Feature::new("colorspace.ICCBased");
