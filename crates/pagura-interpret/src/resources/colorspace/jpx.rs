//! Embedded JPX colour selection (7.4.9; T.801 M.11.7.2).
//!
//! Unsupported profiles and selected transforms advance to the next
//! specification. Malformed profiles and exhausted budgets remain errors.

use super::Colors;
use pagura_color::{ColorSpace, IccSpace, Intent};
use pagura_core::{Error, ErrorKind, ObjectId, Result};
use pagura_image::jpx::JpxColor;
use std::sync::Arc;

impl Colors<'_> {
    pub(crate) fn embedded_jpx(
        &mut self,
        id: ObjectId,
        color: &JpxColor,
        components: u8,
        intent: Intent,
    ) -> Result<Arc<ColorSpace>> {
        match color {
            JpxColor::Alternatives(choices) => {
                for (ordinal, choice) in choices.iter().enumerate() {
                    if let Some(space) =
                        self.jpx_choice(id, ordinal, &choice.color, components, intent)?
                    {
                        return Ok(space);
                    }
                }
            }
            color => {
                if let Some(space) = self.jpx_choice(id, 0, color, components, intent)? {
                    return Ok(space);
                }
            }
        }
        self.jpx_device(components)
    }

    fn jpx_choice(
        &mut self,
        id: ObjectId,
        ordinal: usize,
        color: &JpxColor,
        components: u8,
        intent: Intent,
    ) -> Result<Option<Arc<ColorSpace>>> {
        let data = match color {
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
