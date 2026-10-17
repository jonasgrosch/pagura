//! Retained transparency attributes and blending-space conversion (clause 11).

use crate::blend::{BlendMode, lum};
use crate::color::{Color, Space};
use crate::display::Command;
use crate::functions::Function;
use crate::icc::ColorContext;
use pagura_core::{Dictionary, Error, Result};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub(crate) struct Composite {
    pub(crate) alpha: f64,
    pub(crate) blend: BlendMode,
    pub(crate) mask: Option<Arc<SoftMask>>,
    pub(crate) alpha_is_shape: bool,
    pub(crate) conversion: Conversion,
    pub(crate) overprint: bool,
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
        }
    }
}
impl Composite {
    pub(crate) fn opaque() -> Self {
        Self {
            alpha: 1.0,
            ..Self::default()
        }
    }
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
    pub(crate) fn color(
        self,
        color: &Color,
        conversion: &Conversion,
        work: &mut usize,
    ) -> Result<[f64; 4]> {
        let Color::Solid { space, components } = color else {
            return Err(Error::invalid_argument("pattern is not a solid colour"));
        };
        self.source(space, components, conversion, work)
    }
    pub(crate) fn source(
        self,
        space: &Space,
        components: &[f64],
        conversion: &Conversion,
        work: &mut usize,
    ) -> Result<[f64; 4]> {
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
            let input: Vec<_> = components.iter().map(|v| v.clamp(0.0, 1.0)).collect();
            return self.source(
                base,
                &function.evaluate_with_work(&input, work)?,
                conversion,
                work,
            );
        }
        if let Space::Indexed { base, high, lookup } = space {
            let value = components
                .first()
                .copied()
                .unwrap_or(0.0)
                .round()
                .clamp(0.0, f64::from(*high));
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "index rounded and clamped to 0..255"
            )]
            let index = value as usize;
            let values = lookup
                .get(index * base.components()..(index + 1) * base.components())
                .ok_or(Error::syntax("truncated Indexed colour lookup"))?;
            let decoded = base.decode_lookup(values);
            return self.source(base, &decoded, conversion, work);
        }
        let matching = matches!(
            (self, space),
            (Self::Gray, Space::Gray) | (Self::Rgb, Space::Rgb) | (Self::Cmyk, Space::Cmyk)
        );
        if matching {
            let mut out = [0.0; 4];
            for (out, &v) in out.iter_mut().zip(components) {
                *out = if self == Self::Cmyk {
                    1.0 - v.clamp(0.0, 1.0)
                } else {
                    v.clamp(0.0, 1.0)
                };
            }
            return Ok(out);
        }
        self.encode_rgb(
            space.convert_with_intent(components, conversion.intent)?,
            conversion,
            work,
        )
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
        let mut out = [0.0; 4];
        for (out, &v) in out.iter_mut().zip(value) {
            *out = if self == Self::Cmyk {
                1.0 - v.clamp(0.0, 1.0)
            } else {
                v.clamp(0.0, 1.0)
            };
        }
        Ok(out)
    }
}
fn scalar(function: Option<&Function>, value: f64, work: &mut usize) -> Result<f64> {
    let mut result = [value];
    if let Some(function) = function {
        function.evaluate_into(&[value], &mut result, work)?;
    }
    Ok(result[0])
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
            .map(
                |v| match crate::color::blending(document, resources, v)?.blending_fallback() {
                    Space::Gray => Ok(Model::Gray),
                    Space::Rgb => Ok(Model::Rgb),
                    Space::Cmyk => Ok(Model::Cmyk),
                    _ => Err(Error::unsupported(
                        crate::features::TRANSPARENCY_GROUP_COLORSPACE,
                        "calibrated group blending is not implemented",
                    )),
                },
            )
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
