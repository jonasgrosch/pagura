//! Embedded JPX colour selection (7.4.9; T.801 M.11.7.2).
//!
//! Unsupported profiles and selected transforms advance to the next
//! specification. Malformed profiles and exhausted budgets remain errors.

use super::{Colors, JpxSamples};
use pagura_color::cie::Calibrated;
use pagura_color::{ColorSpace, IccSpace, Intent};
use pagura_core::{Error, ErrorKind, ObjectId, Result};
use pagura_image::jpx::{JpxColor, LabParameters};
use std::sync::Arc;

impl Colors<'_> {
    pub(crate) fn embedded_jpx(
        &mut self,
        id: ObjectId,
        color: &JpxColor,
        bits: &[u8],
        intent: Intent,
        samples: JpxSamples,
    ) -> Result<Arc<ColorSpace>> {
        match color {
            JpxColor::Alternatives(choices) => {
                for (ordinal, choice) in choices.iter().enumerate() {
                    if let Some(space) =
                        self.jpx_choice(id, ordinal, &choice.color, bits, intent, samples)?
                    {
                        return Ok(space);
                    }
                }
            }
            color => {
                if let Some(space) = self.jpx_choice(id, 0, color, bits, intent, samples)? {
                    return Ok(space);
                }
            }
        }
        if samples == JpxSamples::Signed {
            return Err(Error::unsupported(
                pagura_image::features::JPX_COLOUR,
                "signed JPX input requires a restricted XYZ matrix/TRC profile",
            ));
        }
        if color.has_lab() {
            return Err(Error::unsupported(
                pagura_image::features::JPX_COLOUR,
                "JPX CIELab encoding or illuminant is not implemented",
            ));
        }
        self.jpx_device(component_count(bits)?)
    }

    fn jpx_choice(
        &mut self,
        id: ObjectId,
        ordinal: usize,
        color: &JpxColor,
        bits: &[u8],
        intent: Intent,
        samples: JpxSamples,
    ) -> Result<Option<Arc<ColorSpace>>> {
        let components = component_count(bits)?;
        let data = match color {
            JpxColor::Enumerated(_) | JpxColor::Lab(_) if samples == JpxSamples::Signed => {
                return Ok(None);
            }
            JpxColor::Enumerated(14) => return lab(None, bits),
            JpxColor::Lab(parameters) => return lab(Some(parameters), bits),
            JpxColor::Enumerated(12 | 16 | 17) => return self.jpx_device(components).map(Some),
            JpxColor::Enumerated(18) => return Ok(Some(Arc::new(ColorSpace::Sycc))),
            JpxColor::Icc(data) => data,
            _ => return Ok(None),
        };
        let profile = match self
            .profiles
            .embedded(id, ordinal, usize::from(components), data)
        {
            Ok(Some(profile)) => profile,
            Ok(None) => return Ok(None),
            Err(error) if matches!(error.kind, ErrorKind::Unsupported(_)) => return Ok(None),
            Err(error) => return Err(error),
        };
        match profile.check_forward(intent) {
            Ok(()) => {}
            Err(error) if matches!(error.kind, ErrorKind::Unsupported(_)) => return Ok(None),
            Err(error) => return Err(error.in_object(id)),
        }
        // T.800 Table I.9 permits negative-input clipping for these restricted
        // models. General signed ICC input remains undefined; a lower-ranked
        // restricted candidate can still supply a defined interpretation.
        if samples == JpxSamples::Signed && !profile.restricted_trc(intent) {
            return Ok(None);
        }
        // ICC.1:2010 6.3.4 and 10.8: the profile receives physical Lab
        // coordinates, while the JPX sample channel spans its full encoding.
        let range = if profile.lab_input() {
            [[0.0, 100.0], [-128.0, 127.0], [-128.0, 127.0], [0.0, 1.0]]
        } else {
            [[0.0, 1.0]; 4]
        };
        Ok(Some(Arc::new(ColorSpace::Icc(Arc::new(IccSpace {
            profile: Some(profile),
            alternate: self.jpx_device(components)?,
            components,
            range,
            object: Some(id),
        })))))
    }

    /// Container colour and the final fallback do not consult PDF Default
    /// spaces: those are used only by explicit PDF colour descriptions.
    fn jpx_device(&self, components: u8) -> Result<Arc<ColorSpace>> {
        match components {
            1 => Ok(self.device_gray()),
            3 => Ok(self.device_rgb()),
            4 => Ok(self.device_cmyk()),
            _ => Err(Error::unsupported(
                crate::features::XOBJECT_JPX_COLOR,
                "embedded JPX colour component count is not implemented",
            )),
        }
    }
}

fn component_count(bits: &[u8]) -> Result<u8> {
    u8::try_from(bits.len()).map_err(|_| Error::limit("JPX colour component count overflow"))
}

/// T.801 M-18 maps the post-palette unsigned codes to physical Lab intervals.
/// Keeping those intervals on the selected Lab space also supplies image Decode.
fn lab(parameters: Option<&LabParameters>, bits: &[u8]) -> Result<Option<Arc<ColorSpace>>> {
    let &[lightness, a, b] = bits else {
        return Err(Error::syntax("JPX CIELab requires three colour channels"));
    };
    if bits.iter().any(|bits| !(1..=16).contains(bits)) {
        return Err(Error::unsupported(
            pagura_image::features::JPX_COLOUR,
            "JPX CIELab channel precision is not implemented",
        ));
    }
    let (ranges, offsets) = if let Some(parameters) = parameters {
        // Other Table M.29 illuminants and zero ranges have no established
        // interpretation here. They remain candidates for supported fallbacks,
        // not Syntax errors or an inferred device colour space.
        if parameters.illuminant() != 0x0044_3530 || parameters.ranges().contains(&0) {
            return Ok(None);
        }
        (
            parameters.ranges().map(f64::from),
            parameters.offsets().map(f64::from),
        )
    } else {
        // Powers below bit three have fractional offsets, not integer shifts
        // by a negative amount; one- and two-bit colour channels remain valid.
        (
            [100.0, 170.0, 200.0],
            [
                0.0,
                f64::from(1u32 << a) / 2.0,
                3.0 * f64::from(1u32 << b) / 8.0,
            ],
        )
    };
    let maxima = [lightness, a, b].map(|bits| f64::from((1u32 << bits) - 1));
    let intervals = std::array::from_fn(|i| {
        let low = -offsets[i] * ranges[i] / maxima[i];
        [low, low + ranges[i]]
    });
    Ok(Some(Arc::new(ColorSpace::Calibrated(Arc::new(
        Calibrated::lab_with_ranges([0.96422, 1.0, 0.82521], [0.0; 3], intervals)?,
    )))))
}
