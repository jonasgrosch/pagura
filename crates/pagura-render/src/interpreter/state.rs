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
            b"SCN" | b"scn" => {
                color.select_pattern(self.document, resources, v, self.initial_ctm)?;
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
            let replacement = match key.as_bytes() {
                b"TR" => Some(b"TR2".as_slice()),
                b"BG" => Some(b"BG2".as_slice()),
                b"UCR" => Some(b"UCR2".as_slice()),
                _ => None,
            };
            if replacement.is_some_and(|name| dictionary.contains_key(name)) {
                continue;
            }
            if key == b"SMask" {
                self.state.composite.mask = self.soft_mask(value, resources)?;
                continue;
            }
            if matches!(key.as_bytes(), b"BG" | b"BG2" | b"UCR" | b"UCR2") {
                let resolved = self.document.resolve(value)?;
                let function = if resolved.as_name().is_some_and(|n| n == b"Default") {
                    if !matches!(key.as_bytes(), b"BG2" | b"UCR2") {
                        return Err(Error::syntax("Default requires BG2 or UCR2"));
                    }
                    None
                } else {
                    Some(std::sync::Arc::new(crate::functions::Function::parse(
                        self.document,
                        value,
                        1,
                        1,
                    )?))
                };
                if matches!(key.as_bytes(), b"BG" | b"BG2") {
                    self.state.composite.conversion.black = function;
                } else {
                    self.state.composite.conversion.ucr = function;
                }
                continue;
            }
            if key == b"TR"
                && self
                    .document
                    .resolve(value)?
                    .as_name()
                    .is_some_and(|n| n == b"Default")
            {
                return Err(Error::syntax("Default transfer is only valid for TR2"));
            }
            if key == b"TR" || key == b"TR2" {
                self.state.transfer = crate::functions::transfer(self.document, value)?;
                continue;
            }
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
        if dictionary.contains_key(b"OP") && !dictionary.contains_key(b"op") {
            self.state.composite.overprint = self.state.stroke_overprint;
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
                if key == b"CA" {
                    self.state.stroke_alpha = alpha;
                } else {
                    self.state.composite.alpha = alpha;
                }
            }
            b"BM" => {
                self.state.composite.blend = crate::blend::BlendMode::parse(self.document, value)?;
            }
            // Overprint cannot affect an RGB device without separations
            // (8.6.7), but malformed values remain errors.
            b"OP" | b"op" => {
                let flag = value
                    .as_bool()
                    .ok_or(Error::syntax("overprint flag must be boolean"))?;
                if key == b"OP" {
                    self.state.stroke_overprint = flag;
                } else {
                    self.state.composite.overprint = flag;
                }
            }
            b"OPM" => {
                if !matches!(value.as_integer(), Some(0 | 1)) {
                    return Err(Error::syntax("invalid overprint mode"));
                }
            }
            b"AIS" => {
                self.state.composite.alpha_is_shape = value
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
            // 10.4.2: black generation and undercolour removal are used when
            // converting RGB to CMYK. Our RGB output never makes that conversion.
            b"BG" | b"BG2" | b"UCR" | b"UCR2" => {
                let default = matches!(key, b"BG2" | b"UCR2")
                    && value.as_name().is_some_and(|n| n == b"Default");
                if !matches!(value, Value::Dictionary(_)) && !default {
                    return Err(Error::syntax(
                        "black generation or UCR must be a function or Default",
                    ));
                }
            }
            // Continuous-tone output transmits transferred components directly;
            // no screening is performed (10.6.1). Validate the permitted shape.
            b"HT" => {
                if !matches!(value, Value::Dictionary(_))
                    && value.as_name().is_none_or(|n| n != b"Default")
                {
                    return Err(Error::syntax("HT must be a halftone or Default"));
                }
            }
            // Smoothness is a device-dependent approximation hint (8.4.5).
            // Exact geometry uses no shading approximation and may ignore it.
            b"SM" => {
                let smoothness = value
                    .as_number()
                    .ok_or(Error::syntax("SM must be numeric"))?;
                if !(0.0..=1.0).contains(&smoothness) {
                    return Err(Error::syntax("SM outside unit interval"));
                }
            }
            // Text knockout affects glyph compositing only (11.7.4.3). Text
            // operators remain explicitly unsupported, so it cannot affect vectors.
            b"TK" => {
                value.as_bool().ok_or(Error::syntax("TK must be boolean"))?;
            }
            _ => return Err(unsupported(features::EXTGSTATE_UNKNOWN)),
        }
        Ok(())
    }
}
