//! Graphics-state parameters, colour selection and ExtGState (8.4, 8.6).

use super::{
    Color, Compiler, Dictionary, Error, LineCap, LineJoin, Result, Value, color, features, integer,
    number, rendering_intent, resource, unsupported,
};

impl Compiler<'_> {
    pub(super) fn parameter(&mut self, word: &[u8], v: &[Value]) -> Result<()> {
        match word {
            b"w" => self.state.stroke.width = number(v, 0)?.max(0.0),
            b"J" => {
                self.state.stroke.cap = match integer(v, 0)? {
                    0 => LineCap::Butt,
                    1 => LineCap::Round,
                    2 => LineCap::ProjectingSquare,
                    _ => return Err(Error::syntax("invalid line cap")),
                }
            }
            b"j" => {
                self.state.stroke.join = match integer(v, 0)? {
                    0 => LineJoin::Miter,
                    1 => LineJoin::Round,
                    2 => LineJoin::Bevel,
                    _ => return Err(Error::syntax("invalid line join")),
                }
            }
            b"M" => {
                let n = number(v, 0)?;
                if n < 1.0 {
                    return Err(Error::syntax("miter limit must be at least one"));
                }
                self.state.stroke.miter_limit = n;
            }
            b"i" => {
                let n = number(v, 0)?;
                if !(0.0..=100.0).contains(&n) {
                    return Err(Error::syntax("flatness outside range"));
                }
                self.state.flatness = n;
            }
            b"ri" => {
                self.state.intent =
                    rendering_intent(v.first().ok_or(Error::syntax("missing rendering intent"))?)?;
            }
            b"d" => {
                let array = v
                    .first()
                    .and_then(Value::as_array)
                    .ok_or(Error::syntax("dash array expected"))?;
                let dash = array
                    .iter()
                    .map(|n| {
                        n.as_number()
                            .filter(|&n| n >= 0.0)
                            .ok_or(Error::syntax("invalid dash length"))
                    })
                    .collect::<Result<Vec<_>>>()?;
                if !dash.is_empty() && dash.iter().all(|&n| n == 0.0) {
                    return Err(Error::syntax("dash array is all zero"));
                }
                let phase = number(v, 1)?;
                if phase < 0.0 {
                    return Err(Error::syntax("negative dash phase"));
                }
                self.state.stroke.dash_array = dash;
                self.state.stroke.dash_phase = phase;
            }
            _ => return Err(Error::syntax("invalid graphics parameter")),
        }
        if word == b"w" && number(v, 0)? < 0.0 {
            return Err(Error::syntax("negative line width"));
        }
        Ok(())
    }

    pub(super) fn color(&mut self, word: &[u8], v: &[Value], resources: &Dictionary) -> Result<()> {
        let stroking = matches!(word, b"G" | b"RG" | b"K" | b"CS" | b"SC" | b"SCN");
        let color = if stroking {
            &mut self.state.stroke_color
        } else {
            &mut self.state.fill
        };
        match word {
            b"CS" | b"cs" => {
                let name = v
                    .first()
                    .and_then(Value::as_name)
                    .ok_or(Error::syntax("colour space name expected"))?;
                *color = Color::new(color::named(self.document, resources, name.as_bytes())?);
            }
            b"g" | b"G" | b"rg" | b"RG" | b"k" | b"K" => {
                let name = match word {
                    b"g" | b"G" => b"DeviceGray".as_slice(),
                    b"rg" | b"RG" => b"DeviceRGB",
                    _ => b"DeviceCMYK",
                };
                *color = Color::new(color::named(self.document, resources, name)?);
                color.set(v)?;
            }
            _ => color.set(v)?,
        }
        Ok(())
    }

    pub(super) fn extgstate(&mut self, v: &[Value], resources: &Dictionary) -> Result<()> {
        let name = v
            .first()
            .and_then(Value::as_name)
            .ok_or(Error::syntax("ExtGState name expected"))?;
        let value = resource(self.document, resources, b"ExtGState", name.as_bytes())?;
        let dictionary = self.document.resolve_dictionary(value)?;
        for (key, value) in dictionary.iter() {
            let value = self.document.resolve(value)?;
            let operator: Option<&[u8]> = match key.as_bytes() {
                b"LW" => Some(b"w".as_slice()),
                b"LC" => Some(b"J"),
                b"LJ" => Some(b"j"),
                b"ML" => Some(b"M"),
                b"RI" => Some(b"ri"),
                b"FL" => Some(b"i"),
                _ => None,
            };
            if let Some(operator) = operator {
                crate::operators::validate(operator, std::slice::from_ref(value))?;
                self.parameter(operator, std::slice::from_ref(value))?;
                continue;
            }
            self.extgstate_entry(key.as_bytes(), value)?;
        }
        Ok(())
    }

    pub(super) fn extgstate_entry(&mut self, key: &[u8], value: &Value) -> Result<()> {
        match key {
            b"Type" => {
                if value.as_name().is_none_or(|n| n != b"ExtGState") {
                    return Err(Error::syntax("invalid ExtGState Type"));
                }
            }
            b"D" => {
                let array = value
                    .as_array()
                    .filter(|a| a.len() == 2)
                    .ok_or(Error::syntax("invalid ExtGState dash"))?;
                let array = array
                    .iter()
                    .map(|v| self.document.resolve(v).cloned())
                    .collect::<Result<Vec<_>>>()?;
                self.parameter(b"d", &array)?;
            }
            b"SA" => {
                self.state.stroke_adjustment =
                    value.as_bool().ok_or(Error::syntax("SA must be boolean"))?;
            }
            b"CA" | b"ca" => {
                let alpha = value
                    .as_number()
                    .ok_or(Error::syntax("alpha must be numeric"))?;
                if !(0.0..=1.0).contains(&alpha) {
                    return Err(Error::syntax("alpha outside unit interval"));
                }
                if alpha != 1.0 {
                    return Err(unsupported(if key == b"CA" {
                        features::TRANSPARENCY_CA_STROKING
                    } else {
                        features::TRANSPARENCY_CA
                    }));
                }
            }
            b"BM" => {
                let name = value.as_name().or_else(|| {
                    value
                        .as_array()
                        .and_then(|a| a.first())
                        .and_then(Value::as_name)
                });
                if name.is_none_or(|n| n != b"Normal") {
                    return Err(unsupported(features::TRANSPARENCY_BM));
                }
            }
            b"SMask" => {
                if value.as_name().is_none_or(|n| n != b"None") {
                    return Err(unsupported(features::TRANSPARENCY_SMASK));
                }
            }
            // Overprint cannot affect an RGB device without separations
            // (8.6.7), but malformed values remain errors.
            b"OP" | b"op" => {
                value
                    .as_bool()
                    .ok_or(Error::syntax("overprint flag must be boolean"))?;
            }
            b"OPM" => {
                if !matches!(value.as_integer(), Some(0 | 1)) {
                    return Err(Error::syntax("invalid overprint mode"));
                }
            }
            b"AIS" => {
                value
                    .as_bool()
                    .ok_or(Error::syntax("AIS must be boolean"))?;
            }
            b"TR" | b"TR2" => {
                if value
                    .as_name()
                    .is_none_or(|n| n != b"Identity" && !(key == b"TR2" && n == b"Default"))
                {
                    return Err(unsupported(features::EXTGSTATE_TR));
                }
            }
            b"Font" => return Err(unsupported(features::EXTGSTATE_FONT)),
            b"BG" | b"BG2" => return Err(unsupported(features::EXTGSTATE_BG)),
            b"UCR" | b"UCR2" => return Err(unsupported(features::EXTGSTATE_UCR)),
            b"HT" => return Err(unsupported(features::EXTGSTATE_HT)),
            b"SM" => return Err(unsupported(features::EXTGSTATE_SM)),
            b"TK" => return Err(unsupported(features::EXTGSTATE_TK)),
            _ => return Err(unsupported(features::EXTGSTATE_UNKNOWN)),
        }
        Ok(())
    }
}
