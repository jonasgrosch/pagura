//! Transparency group attributes (11.6.6, Table 145) and blending spaces
//! (11.3.4).

use super::colorspace::Colors;
use crate::features;
use crate::model::GroupAttributes;
use pagura_color::{ColorSpace, IccSpace, Process};
use pagura_core::{Dictionary, Error, Value};
use pagura_document::source::ReadResult;

/// The attributes of the `Group` entry of `dictionary`, if any.
///
/// # Errors
///
/// `Unsupported` for groups other than transparency groups and for blending
/// spaces that need an unsupported colour transform; `Syntax` for malformed
/// entries; and colour-space errors.
pub(crate) fn attributes(
    colors: &mut Colors<'_>,
    dictionary: &Dictionary,
    resources: &Dictionary,
) -> ReadResult<Option<GroupAttributes>> {
    let document = colors.document();
    let Some(value) = dictionary.get(b"Group") else {
        return Ok(None);
    };
    let d = document.resolve_dictionary(value)?;
    if !d.is_name(b"S", b"Transparency") {
        return Err(
            Error::unsupported(features::TRANSPARENCY_GROUP, "unknown group subtype").into(),
        );
    }
    let space = d
        .get(b"CS")
        .map(|v| blending_space(colors, resources, v))
        .transpose()?;
    let boolean = |key: &[u8]| -> ReadResult<bool> {
        d.get(key)
            .map(|v| {
                document
                    .resolve(v)?
                    .as_bool()
                    .ok_or(Error::syntax("group flag must be boolean"))
            })
            .transpose()
            .map(|v| v.unwrap_or(false))
    };
    Ok(Some(GroupAttributes {
        isolated: boolean(b"I")?,
        knockout: boolean(b"K")?,
        space,
    }))
}

/// The device family a group colour space composites in: device spaces
/// themselves, and ICCBased spaces without a Lab-input profile in their
/// component count's family (there are no inverse ICC transforms). An
/// ICCBased space whose profile is not supported uses its alternate.
fn blending_space(
    colors: &mut Colors<'_>,
    resources: &Dictionary,
    value: &Value,
) -> ReadResult<Process> {
    let space = colors.resolve(resources, value)?;
    let space = match space.as_ref() {
        ColorSpace::Icc(icc) if icc.profile.is_none() => icc.alternate.as_ref(),
        space => space,
    };
    match space {
        ColorSpace::Icc(icc) if icc.profile.as_ref().is_none_or(|p| !p.lab_input()) => {
            icc_family(icc)
        }
        space => device(space),
    }
    .ok_or(Error::unsupported(
        features::TRANSPARENCY_GROUP_COLORSPACE,
        "group blending space requires an unsupported colour transform",
    ))
}

fn device(space: &ColorSpace) -> Option<Process> {
    match space {
        ColorSpace::DeviceGray => Some(Process::Gray),
        ColorSpace::DeviceRgb => Some(Process::Rgb),
        ColorSpace::DeviceCmyk => Some(Process::Cmyk),
        _ => None,
    }
}

/// An ICCBased space's device family: its component count's family when its
/// profile is used, its alternate's otherwise.
fn icc_family(icc: &IccSpace) -> Option<Process> {
    if icc.profile.is_some() {
        match icc.components {
            1 => Some(Process::Gray),
            3 => Some(Process::Rgb),
            _ => Some(Process::Cmyk),
        }
    } else {
        device(&icc.alternate)
    }
}
