//! Graphics state parameter dictionaries (8.4.5, Table 58). The font and
//! the entries that operators also set apply for every device; devices
//! without paint read no other entry.

use super::{Machine, Scope, params};
use crate::device::Device;
use crate::features;
use crate::model::BlendMode;
use crate::resources::function;
use pagura_content::{Dash, Op};
use pagura_core::{Dictionary, Error, Name, Result, Value};
use pagura_document::Document;

impl<D: Device + ?Sized> Machine<'_, '_, D> {
    /// `gs`.
    pub(super) fn graphics_state(
        &mut self,
        name: &Name,
        resources: &Dictionary,
        scope: &mut Scope,
    ) -> Result<()> {
        let document = self.document();
        let Some(value) = self.named_resource(resources, b"ExtGState", name.as_bytes())? else {
            return Ok(());
        };
        let dictionary = document.resolve_dictionary(value)?;
        let uncolored = scope.uncolored_glyph();
        if let Some(font) = dictionary.get(b"Font") {
            let array = document
                .resolve(font)?
                .as_array()
                .filter(|a| a.len() == 2)
                .ok_or(Error::syntax("ExtGState Font requires font and size"))?;
            let size = document
                .resolve(&array[1])?
                .as_number()
                .ok_or(Error::syntax("invalid ExtGState font size"))?;
            self.select_font(&array[0], size, scope)?;
        }
        let paint = self.needs().paint;
        if paint && let Some(knockout) = dictionary.get(b"TK") {
            let knockout = document
                .resolve(knockout)?
                .as_bool()
                .ok_or(Error::syntax("TK must be boolean"))?;
            if scope.text.is_none() && !uncolored {
                scope.state.text.knockout = knockout;
            }
        }
        for (key, value) in dictionary {
            let key = key.as_bytes();
            // A Type 3 glyph declared with d1 ignores colour parameters
            // (9.6.4, Table 111).
            if !defined_entry(key)
                || key == b"Font"
                || (uncolored && color_parameter(key))
                || (!paint && !operator_entry(key))
            {
                continue;
            }
            self.entry(dictionary, key, value, resources, scope)?;
        }
        if paint && !uncolored && dictionary.contains_key(b"OP") && !dictionary.contains_key(b"op")
        {
            scope.state.fill_overprint = scope.state.stroke_overprint;
        }
        scope.state.composite_changed();
        Ok(())
    }

    fn entry(
        &mut self,
        dictionary: &Dictionary,
        key: &[u8],
        value: &Value,
        resources: &Dictionary,
        scope: &mut Scope,
    ) -> Result<()> {
        let document = self.document();
        if scope.stencil
            && matches!(
                key,
                b"TR" | b"TR2" | b"BG" | b"BG2" | b"UCR" | b"UCR2" | b"HT" | b"UseBlackPtComp"
            )
        {
            return Ok(());
        }
        let replacement: Option<&[u8]> = match key {
            b"TR" => Some(b"TR2"),
            b"BG" => Some(b"BG2"),
            b"UCR" => Some(b"UCR2"),
            _ => None,
        };
        if replacement.is_some_and(|name| dictionary.contains_key(name)) {
            return Ok(());
        }
        match key {
            b"SMask" => {
                scope.state.soft_mask = self.soft_mask(value, resources, scope)?;
            }
            b"BG" | b"BG2" | b"UCR" | b"UCR2" => {
                let function = if document
                    .resolve(value)?
                    .as_name()
                    .is_some_and(|n| n == b"Default")
                {
                    if !matches!(key, b"BG2" | b"UCR2") {
                        return Err(Error::syntax("Default requires BG2 or UCR2"));
                    }
                    None
                } else {
                    Some(function::scalar(document, value)?)
                };
                if matches!(key, b"BG" | b"BG2") {
                    scope.state.black_generation = function;
                } else {
                    scope.state.undercolor_removal = function;
                }
            }
            b"TR" | b"TR2" => {
                if key == b"TR"
                    && document
                        .resolve(value)?
                        .as_name()
                        .is_some_and(|n| n == b"Default")
                {
                    return Err(Error::syntax("Default transfer is only valid for TR2"));
                }
                scope.state.transfer = function::transfer(document, value)?;
            }
            _ => {
                let value = document.resolve(value)?;
                // The entries of the operators that set the same parameters,
                // with their operand checks.
                let wrong = || Error::syntax("wrong operand type");
                let number = || value.as_number().ok_or_else(wrong);
                let integer = || value.as_integer().ok_or_else(wrong);
                let op = match key {
                    b"LW" => Op::LineWidth(number()?),
                    b"LC" => Op::LineCap(integer()?),
                    b"LJ" => Op::LineJoin(integer()?),
                    b"ML" => Op::MiterLimit(number()?),
                    b"RI" => Op::RenderingIntent(value.as_name().cloned().ok_or_else(wrong)?),
                    b"FL" => Op::Flatness(number()?),
                    _ => return parameter(document, key, value, scope),
                };
                return params::apply(&op, &mut scope.state);
            }
        }
        scope.state.composite_changed();
        Ok(())
    }
}

/// The entries Table 58 defines that need no function, colour space or
/// soft mask.
fn parameter(document: &Document, key: &[u8], value: &Value, scope: &mut Scope) -> Result<()> {
    let state = &mut scope.state;
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
            let lengths = document
                .resolve(&array[0])?
                .as_array()
                .ok_or(Error::syntax("dash array expected"))?;
            let phase = document
                .resolve(&array[1])?
                .as_number()
                .ok_or(Error::syntax("numeric operand expected"))?;
            params::apply(&Op::Dash(Dash::new(lengths, phase)?), state)?;
        }
        b"SA" => {
            state.adjustment = value.as_bool().ok_or(Error::syntax("SA must be boolean"))?;
            state.pen_changed();
        }
        b"CA" | b"ca" => {
            let alpha = value
                .as_number()
                .ok_or(Error::syntax("alpha must be numeric"))?
                .clamp(0.0, 1.0);
            if key == b"CA" {
                state.stroke_alpha = alpha;
            } else {
                state.fill_alpha = alpha;
            }
        }
        b"BM" => state.blend = blend_mode(document, value)?,
        // Both flags are kept; OP supplies op only when the dictionary omits
        // op (Table 57).
        b"OP" | b"op" => {
            let flag = value
                .as_bool()
                .ok_or(Error::syntax("overprint flag must be boolean"))?;
            if key == b"OP" {
                state.stroke_overprint = flag;
            } else {
                state.fill_overprint = flag;
            }
        }
        b"OPM" => {
            if !matches!(value.as_integer(), Some(0 | 1)) {
                return Err(Error::syntax("invalid overprint mode"));
            }
            state.nonzero_overprint = value.as_integer() == Some(1);
        }
        b"AIS" => {
            state.alpha_is_shape = value
                .as_bool()
                .ok_or(Error::syntax("AIS must be boolean"))?;
        }
        // Continuous-tone output transmits transferred components directly;
        // no screening is performed (10.6.1).
        b"HT" => {
            if !matches!(value, Value::Dictionary(_))
                && value.as_name().is_none_or(|n| n != b"Default")
            {
                return Err(Error::syntax("HT must be a halftone or Default"));
            }
        }
        // Smoothness is a device-dependent approximation hint (8.4.5).
        b"SM" => {
            value
                .as_number()
                .ok_or(Error::syntax("SM must be numeric"))?;
        }
        // Text knockout was applied before the other entries.
        b"TK" => {
            value.as_bool().ok_or(Error::syntax("TK must be boolean"))?;
        }
        b"UseBlackPtComp" => {
            let name = value
                .as_name()
                .ok_or(Error::syntax("UseBlackPtComp must be a name"))?;
            if name == b"ON" {
                return Err(Error::unsupported(
                    features::EXTGSTATE_BLACK_POINT_COMPENSATION,
                    "rendering feature is not implemented",
                ));
            }
            if name != b"OFF" && name != b"Default" {
                return Err(Error::syntax("invalid UseBlackPtComp"));
            }
        }
        // Continuous-tone output has no halftone grid (10.6.1, Table 58).
        b"HTO"
            if value
                .as_array()
                .is_none_or(|v| v.len() != 2 || v.iter().any(|v| v.as_number().is_none())) =>
        {
            return Err(Error::syntax("HTO requires two numbers"));
        }
        _ => {}
    }
    state.composite_changed();
    Ok(())
}

/// `BM`: a name or an array whose first known name applies (11.3.5).
fn blend_mode(document: &Document, value: &Value) -> Result<BlendMode> {
    if let Some(names) = value.as_array() {
        let mut selected = None;
        for value in names {
            let name = document
                .resolve(value)?
                .as_name()
                .ok_or(Error::syntax("blend array requires names"))?;
            if selected.is_none() {
                selected = named_blend(name.as_bytes());
            }
        }
        return Ok(selected.unwrap_or_default());
    }
    let name = value
        .as_name()
        .ok_or(Error::syntax("blend mode must be a name or array"))?;
    named_blend(name.as_bytes()).ok_or(Error::unsupported(
        features::TRANSPARENCY_BM,
        "unknown blend mode",
    ))
}

fn named_blend(name: &[u8]) -> Option<BlendMode> {
    Some(match name {
        b"Normal" | b"Compatible" => BlendMode::Normal,
        b"Multiply" => BlendMode::Multiply,
        b"Screen" => BlendMode::Screen,
        b"Overlay" => BlendMode::Overlay,
        b"Darken" => BlendMode::Darken,
        b"Lighten" => BlendMode::Lighten,
        b"ColorDodge" => BlendMode::ColorDodge,
        b"ColorBurn" => BlendMode::ColorBurn,
        b"HardLight" => BlendMode::HardLight,
        b"SoftLight" => BlendMode::SoftLight,
        b"Difference" => BlendMode::Difference,
        b"Exclusion" => BlendMode::Exclusion,
        b"Hue" => BlendMode::Hue,
        b"Saturation" => BlendMode::Saturation,
        b"Color" => BlendMode::Color,
        b"Luminosity" => BlendMode::Luminosity,
        _ => return None,
    })
}

/// Table 58 is exhaustive for standard entries. Extension entries are
/// skipped before resolution, so an ignored producer key loads no object.
fn defined_entry(key: &[u8]) -> bool {
    matches!(
        key,
        b"Type"
            | b"LW"
            | b"LC"
            | b"LJ"
            | b"ML"
            | b"D"
            | b"RI"
            | b"OP"
            | b"op"
            | b"OPM"
            | b"Font"
            | b"BG"
            | b"BG2"
            | b"UCR"
            | b"UCR2"
            | b"TR"
            | b"TR2"
            | b"HT"
            | b"FL"
            | b"SM"
            | b"SA"
            | b"BM"
            | b"SMask"
            | b"CA"
            | b"ca"
            | b"AIS"
            | b"TK"
            | b"UseBlackPtComp"
            | b"HTO"
    )
}

/// The entries that set what operators also set: the line parameters (with
/// stroke adjustment), flatness and the rendering intent (8.4.3, 8.6.5.8).
fn operator_entry(key: &[u8]) -> bool {
    matches!(
        key,
        b"LW" | b"LC" | b"LJ" | b"ML" | b"D" | b"SA" | b"FL" | b"RI"
    )
}

/// The entries a Type 3 glyph declared with `d1` ignores.
fn color_parameter(key: &[u8]) -> bool {
    matches!(
        key,
        b"RI"
            | b"BG"
            | b"BG2"
            | b"UCR"
            | b"UCR2"
            | b"TR"
            | b"TR2"
            | b"HT"
            | b"UseBlackPtComp"
            | b"CA"
            | b"ca"
            | b"BM"
            | b"SMask"
            | b"AIS"
            | b"OP"
            | b"op"
            | b"OPM"
            | b"TK"
    )
}
