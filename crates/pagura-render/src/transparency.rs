//! Retained transparency attributes and blending-space conversion (clause 11).

use crate::blend::{BlendMode, lum};
use crate::color::Space;
use crate::display::Command;
use crate::functions::Function;
use crate::icc::ColorContext;
use pagura_core::{Dictionary, Error, Result, Value};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub(crate) struct Composite {
    pub(crate) alpha: f64,
    pub(crate) blend: BlendMode,
    pub(crate) mask: Option<Arc<SoftMask>>,
    pub(crate) alpha_is_shape: bool,
    pub(crate) conversion: Conversion,
    pub(crate) overprint: bool,
    pub(crate) nonzero_overprint: bool,
}
impl Default for Composite {
    fn default() -> Self {
        Self {
            alpha: 1.0,
            blend: BlendMode::Normal,
            mask: None,
            alpha_is_shape: false,
            conversion: Conversion::default(),
            overprint: false,
            nonzero_overprint: false,
        }
    }
}
impl Composite {
    pub(crate) fn simple(&self) -> bool {
        self.alpha == 1.0 && self.blend == BlendMode::Normal && self.mask.is_none()
    }
}

/// Device policy for RGB-to-CMYK conversion (10.3.4): default black generation
/// and undercolour removal both use the common ink amount. Explicit BG/UCR
/// functions replace that policy when a CMYK blending space requires conversion.
#[derive(Debug, Clone, Default)]
pub(crate) struct Conversion {
    pub(crate) intent: crate::icc::Intent,
    pub(crate) black: Option<Arc<Function>>,
    pub(crate) ucr: Option<Arc<Function>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Model {
    Gray,
    #[default]
    Rgb,
    Cmyk,
}

/// Native component conversion and exact zero selection share the same clipping.
/// Keep zero selection before complementing: a tiny nonzero tint can round to
/// additive white, but must still replace its backdrop component (11.7.4.3).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Native {
    pub(crate) color: [f64; 4],
    pub(crate) zero: [bool; 4],
}

impl Model {
    pub(crate) fn components(self) -> usize {
        match self {
            Self::Gray => 1,
            Self::Rgb => 3,
            Self::Cmyk => 4,
        }
    }
    pub(crate) fn rgb(self, c: [f64; 4]) -> [f64; 3] {
        match self {
            Self::Rgb => [c[0], c[1], c[2]],
            Self::Gray => [c[0]; 3],
            Self::Cmyk => [0, 1, 2].map(|i| (c[i] + c[3] - 1.0).max(0.0)),
        }
    }
    /// Device-gray conversion for mask luminosity (11.5.3). CMYK uses
    /// weighted ink coverage before clipping, preserving the black component.
    pub(crate) fn luminosity(self, c: [f64; 4]) -> f64 {
        match self {
            Self::Gray => c[0],
            Self::Rgb => lum([c[0], c[1], c[2]]),
            Self::Cmyk => (lum([c[0], c[1], c[2]]) + c[3] - 1.0).max(0.0),
        }
    }
    pub(crate) fn encode_rgb(
        self,
        rgb: [f64; 3],
        conversion: &Conversion,
        work: &mut usize,
    ) -> Result<[f64; 4]> {
        Ok(match self {
            Self::Rgb => [rgb[0], rgb[1], rgb[2], 0.0],
            Self::Gray => [lum(rgb), 0.0, 0.0, 0.0],
            Self::Cmyk => {
                let inks = rgb.map(|v| 1.0 - v);
                let amount = inks.into_iter().fold(1.0, f64::min);
                let black = scalar(conversion.black.as_deref(), amount, work)?.clamp(0.0, 1.0);
                let removal = scalar(conversion.ucr.as_deref(), amount, work)?.clamp(-1.0, 1.0);
                [
                    1.0 - (inks[0] - removal).clamp(0.0, 1.0),
                    1.0 - (inks[1] - removal).clamp(0.0, 1.0),
                    1.0 - (inks[2] - removal).clamp(0.0, 1.0),
                    1.0 - black,
                ]
            }
        })
    }
    pub(crate) fn source(
        self,
        space: &Space,
        components: &[f64],
        conversion: &Conversion,
        work: &mut usize,
    ) -> Result<[f64; 4]> {
        if let Some(native) = self.native(space, components) {
            return Ok(native.color);
        }
        if let Space::Special { all: true, .. } = space {
            let value = 1.0 - components.first().copied().unwrap_or(1.0).clamp(0.0, 1.0);
            return Ok([value; 4]);
        }
        if let Space::Icc {
            base,
            profile: None,
            range,
        } = space
        {
            let clipped = Space::clip_icc(components, range);
            return self.source(base, &clipped[..base.components()], conversion, work);
        }
        if let Space::Tint { base, function, .. } = space {
            let alternate = crate::color::tint(function, components, work)?;
            return self.source(base, &alternate, conversion, work);
        }
        if let Some((base, decoded)) = indexed(space, components)? {
            return self.source(base, &decoded, conversion, work);
        }
        self.encode_rgb(
            space.convert(components, conversion.intent, work)?,
            conversion,
            work,
        )
    }
    pub(crate) fn native(self, space: &Space, components: &[f64]) -> Option<Native> {
        if let Space::Icc {
            base,
            profile: None,
            range,
        } = space
        {
            let clipped = Space::clip_icc(components, range);
            return self.native(base, &clipped[..base.components()]);
        }
        if !matches!(
            (self, space),
            (Self::Gray, Space::Gray) | (Self::Rgb, Space::Rgb) | (Self::Cmyk, Space::Cmyk)
        ) {
            return None;
        }
        Some(self.native_values(components))
    }
    fn native_values(self, components: &[f64]) -> Native {
        let mut native = Native {
            color: [0.0; 4],
            zero: [false; 4],
        };
        for ((color, zero), &v) in native
            .color
            .iter_mut()
            .zip(&mut native.zero)
            .zip(components)
        {
            let v = v.clamp(0.0, 1.0);
            *zero = v == 0.0;
            *color = if self == Self::Cmyk { 1.0 - v } else { v };
        }
        native
    }
    pub(crate) fn black(self) -> [f64; 4] {
        if self == Self::Cmyk {
            [1.0, 1.0, 1.0, 0.0]
        } else {
            [0.0; 4]
        }
    }
    pub(crate) fn values(self, value: &[f64]) -> Result<[f64; 4]> {
        if value.len() != self.components() {
            return Err(Error::syntax("wrong group backdrop component count"));
        }
        Ok(self.native_values(value).color)
    }

    fn parse(document: &ColorContext<'_>, resources: &Dictionary, value: &Value) -> Result<Self> {
        let space = crate::color::blending(document, resources, value)?.blending_fallback();
        match space {
            Space::Gray => Ok(Self::Gray),
            Space::Rgb => Ok(Self::Rgb),
            Space::Cmyk => Ok(Self::Cmyk),
            Space::Icc { base, profile, .. } if profile.as_ref().is_none_or(|p| !p.lab_input()) => {
                // Without inverse ICC transforms a supported ICC group
                // composites in its N-component device base; sources keep
                // their forward transforms.
                match base.as_ref() {
                    Space::Gray => Ok(Self::Gray),
                    Space::Rgb => Ok(Self::Rgb),
                    Space::Cmyk => Ok(Self::Cmyk),
                    _ => Err(group_space_error()),
                }
            }
            _ => Err(group_space_error()),
        }
    }
}

/// Resolve an Indexed sample before colour conversion and component selection
/// (8.6.6.3). Both consumers use the same clipping and base-space decoding.
pub(crate) fn indexed<'a>(
    space: &'a Space,
    components: &[f64],
) -> Result<Option<(&'a Space, crate::color::Components)>> {
    let Space::Indexed { base, high, lookup } = space else {
        return Ok(None);
    };
    let value = components.first().copied().unwrap_or(0.0);
    Ok(Some((base, Space::lookup(base, *high, lookup, value)?)))
}

fn group_space_error() -> Error {
    Error::unsupported(
        crate::features::TRANSPARENCY_GROUP_COLORSPACE,
        "group blending space requires an unsupported colour transform",
    )
}

fn scalar(function: Option<&Function>, value: f64, work: &mut usize) -> Result<f64> {
    function.map_or(Ok(value), |function| function.evaluate_scalar(value, work))
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ViewFit {
    pub(crate) anchor: pagura_raster::Point,
    pub(crate) no_zoom: bool,
    pub(crate) no_rotate: bool,
}
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Attributes {
    pub(crate) view: Option<ViewFit>,
    pub(crate) model: Option<Model>,
    pub(crate) isolated: bool,
    pub(crate) knockout: bool,
}
impl Attributes {
    pub(crate) fn parse(
        document: &ColorContext<'_>,
        dictionary: &Dictionary,
        resources: &Dictionary,
    ) -> Result<Option<Self>> {
        let Some(value) = dictionary.get(b"Group") else {
            return Ok(None);
        };
        let d = document.resolve_dictionary(value)?;
        if !d.is_name(b"S", b"Transparency") {
            return Err(Error::unsupported(
                crate::features::TRANSPARENCY_GROUP,
                "unknown group subtype",
            ));
        }
        let boolean = |key: &[u8]| {
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
        let model = d
            .get(b"CS")
            .map(|v| Model::parse(document, resources, v))
            .transpose()?;
        Ok(Some(Self {
            view: None,
            model,
            isolated: boolean(b"I")?,
            knockout: boolean(b"K")?,
        }))
    }
}

#[derive(Debug)]
pub(crate) struct Group {
    pub(crate) position: pagura_core::Position,
    pub(crate) bounds: Option<Arc<pagura_raster::Path>>,
    pub(crate) commands: Vec<Command>,
    pub(crate) attributes: Attributes,
    pub(crate) composite: Composite,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum MaskMethod {
    Alpha,
    Luminosity,
}
#[derive(Debug)]
pub(crate) struct SoftMask {
    pub(crate) group: Group,
    pub(crate) method: MaskMethod,
    pub(crate) backdrop: [f64; 4],
    pub(crate) transfer: Option<Function>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_zero_selection_precedes_complement_rounding() {
        let native = Model::Cmyk
            .native(&Space::Cmyk, &[0.5, 1e-20, -1.0, 2.0])
            .unwrap();
        assert_eq!(native.color, [0.5, 1.0, 1.0, 0.0]);
        assert_eq!(native.zero, [false, false, true, false]);
    }
}
