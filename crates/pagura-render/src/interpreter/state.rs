//! Graphics-state parameters, colour selection and ExtGState (8.4, 8.6).

use super::{
    Color, Compiler, Dictionary, Error, LineCap, LineJoin, Name, Result, Value, color, features,
    rendering_intent, resource, unsupported,
};
use crate::operators::{ColorOperation, Operation, Operator, Parameter, Target};

impl Compiler<'_> {
    pub(super) fn parameter(&mut self, parameter: Parameter<'_>) -> Result<()> {
        match parameter {
            Parameter::LineWidth(width) => self.state.stroke.width = width.max(0.0),
            Parameter::LineCap(cap) => {
                self.state.stroke.cap = match cap.clamp(0, 2) {
                    0 => LineCap::Butt,
                    1 => LineCap::Round,
                    _ => LineCap::ProjectingSquare,
                }
            }
            Parameter::LineJoin(join) => {
                self.state.stroke.join = match join.clamp(0, 2) {
                    0 => LineJoin::Miter,
                    1 => LineJoin::Round,
                    _ => LineJoin::Bevel,
                }
            }
            Parameter::MiterLimit(limit) => self.state.stroke.miter_limit = limit.max(1.0),
            Parameter::Flatness(flatness) => self.state.flatness = flatness.clamp(0.0, 100.0),
            Parameter::Intent(name) => {
                self.state.intent = rendering_intent(name);
                self.state.composite.conversion.intent =
                    crate::icc::Intent::from_name(&self.state.intent);
            }
            Parameter::Dash { array, phase } => {
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
                self.state.stroke.dash_array = dash;
                self.state.stroke.dash_phase = phase;
            }
        }
        Ok(())
    }

    pub(super) fn color(
        &mut self,
        target: Target,
        operation: ColorOperation<'_>,
        resources: &Dictionary,
    ) -> Result<()> {
        let context = crate::icc::ColorContext::new(self.document, &self.compilation.colors);
        let color = match target {
            Target::Stroke => &mut self.state.stroke_color,
            Target::Fill => &mut self.state.fill,
        };
        match operation {
            ColorOperation::Space(name) => {
                *color = Color::new(color::named(&context, resources, name.as_bytes())?);
            }
            ColorOperation::Device(device, v) => {
                *color = Color::new(color::named(&context, resources, device.name())?);
                color.set(v)?;
            }
            ColorOperation::ComponentsOrPattern(v) => {
                color.select_pattern(&context, resources, v, self.initial_ctm, self.position)?;
            }
            ColorOperation::Components(v) => {
                color.validate_simple_operator(self.document, v, self.position)?;
                color.set(v)?;
            }
        }
        Ok(())
    }

    /// `gs` (8.4.5): `Font` selects a font and `TK` sets text knockout
    /// directly; the other entries apply through [`Self::extgstate`].
    pub(super) fn graphics_state(&mut self, name: &Name, resources: &Dictionary) -> Result<()> {
        let value = resource(self.document, resources, b"ExtGState", name.as_bytes())?;
        let dictionary = self.document.resolve_dictionary(value)?;
        let uncolored = self.uncolored_glyph();
        if let Some(font) = dictionary.get(b"Font") {
            let [font, size] = self
                .document
                .resolve(font)?
                .as_array()
                .and_then(|a| <&[Value; 2]>::try_from(a).ok())
                .ok_or(Error::syntax("ExtGState Font requires font and size"))?;
            let size = self
                .document
                .resolve(size)?
                .as_number()
                .ok_or(Error::syntax("invalid ExtGState font size"))?;
            self.select_font(font, size)?;
        }
        if let Some(knockout) = dictionary.get(b"TK") {
            let knockout = self
                .document
                .resolve(knockout)?
                .as_bool()
                .ok_or(Error::syntax("TK must be boolean"))?;
            if self.state.text.object.is_none() && !uncolored {
                self.state.text.knockout = knockout;
            }
        }
        self.extgstate(dictionary, resources)
    }

    /// Applies the entries of an ExtGState dictionary (Table 58) that the
    /// current context does not ignore (see [`Self::ignores_entry`]).
    fn extgstate(&mut self, dictionary: &Dictionary, resources: &Dictionary) -> Result<()> {
        let ignored = self.ignored_entries();
        let present = |key: &[u8]| dictionary.contains_key(key) && !ignored.contains(key);
        let overprint = present(b"OP") && !present(b"op");
        for (key, value) in dictionary {
            if !defined_entry(key.as_bytes()) || ignored.contains(key.as_bytes()) {
                continue;
            }
            let replacement = match key.as_bytes() {
                b"TR" => Some(b"TR2".as_slice()),
                b"BG" => Some(b"BG2".as_slice()),
                b"UCR" => Some(b"UCR2".as_slice()),
                _ => None,
            };
            if replacement.is_some_and(present) {
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
            // Table 58 entries that set an operator's parameter.
            let operator = match key.as_bytes() {
                b"LW" => Some(Operator::LineWidth),
                b"LC" => Some(Operator::LineCap),
                b"LJ" => Some(Operator::LineJoin),
                b"ML" => Some(Operator::MiterLimit),
                b"RI" => Some(Operator::Intent),
                b"FL" => Some(Operator::Flatness),
                _ => None,
            };
            if let Some(operator) = operator {
                if let Operation::Parameter(parameter) =
                    operator.decode(std::slice::from_ref(value))?
                {
                    self.parameter(parameter)?;
                }
                continue;
            }
            self.extgstate_entry(key.as_bytes(), value)?;
        }
        if overprint {
            self.state.composite.overprint = self.state.stroke_overprint;
        }
        Ok(())
    }

    fn ignored_entries(&self) -> IgnoredEntries {
        IgnoredEntries {
            uncolored_glyph: self.uncolored_glyph(),
            uncolored_cell: self.stencil_pattern,
        }
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
                if let Operation::Parameter(parameter) = Operator::Dash.decode(&array)? {
                    self.parameter(parameter)?;
                }
            }
            b"SA" => {
                self.state.stroke_adjustment =
                    value.as_bool().ok_or(Error::syntax("SA must be boolean"))?;
            }
            b"CA" | b"ca" => {
                let alpha = value
                    .as_number()
                    .ok_or(Error::syntax("alpha must be numeric"))?
                    .clamp(0.0, 1.0);
                if key == b"CA" {
                    self.state.stroke_alpha = alpha;
                } else {
                    self.state.composite.alpha = alpha;
                }
            }
            b"BM" => {
                self.state.composite.blend = crate::blend::BlendMode::parse(self.document, value)?;
            }
            // Retain both flags independently; OP supplies op only when the
            // dictionary omits op (Table 57).
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
                self.state.composite.nonzero_overprint = value.as_integer() == Some(1);
            }
            b"AIS" => {
                self.state.composite.alpha_is_shape = value
                    .as_bool()
                    .ok_or(Error::syntax("AIS must be boolean"))?;
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
                value
                    .as_number()
                    .ok_or(Error::syntax("SM must be numeric"))?;
            }
            // Text knockout affects glyph compositing (9.3.8; 11.7.4.3).
            // The text-state interpreter applies it before this vector-side validation.
            b"TK" => {
                value.as_bool().ok_or(Error::syntax("TK must be boolean"))?;
            }
            _ => return device_parameter(key, value),
        }
        Ok(())
    }
}

fn device_parameter(key: &[u8], value: &Value) -> Result<()> {
    match key {
        // Table 58 defines all recognized keys. 7.3.7 permits extension
        // entries such as producer metadata; they do not alter graphics.
        b"UseBlackPtComp" => {
            let name = value
                .as_name()
                .ok_or(Error::syntax("UseBlackPtComp must be a name"))?;
            if name == b"ON" {
                return Err(unsupported(features::EXTGSTATE_BLACK_POINT_COMPENSATION));
            }
            if name != b"OFF" && name != b"Default" {
                return Err(Error::syntax("invalid UseBlackPtComp"));
            }
        }
        // Continuous-tone output has no halftone grid (10.6.1; Table 58).
        b"HTO"
            if value
                .as_array()
                .is_none_or(|v| v.len() != 2 || v.iter().any(|v| v.as_number().is_none())) =>
        {
            return Err(Error::syntax("HTO requires two numbers"));
        }
        _ => {}
    }
    Ok(())
}

/// ExtGState entries a context ignores: `Font`, which `gs` applies itself;
/// colour-related entries inside an uncoloured Type 3 glyph (9.6.4); device
/// colour-rendering entries inside an uncoloured tiling cell (8.6.8).
#[derive(Debug, Clone, Copy)]
struct IgnoredEntries {
    uncolored_glyph: bool,
    uncolored_cell: bool,
}
impl IgnoredEntries {
    fn contains(self, key: &[u8]) -> bool {
        key == b"Font"
            || (self.uncolored_glyph && color_entry(key))
            || (self.uncolored_cell
                && matches!(
                    key,
                    b"TR" | b"TR2" | b"BG" | b"BG2" | b"UCR" | b"UCR2" | b"HT" | b"UseBlackPtComp"
                ))
    }
}

/// The colour-related ExtGState entries an uncoloured Type 3 glyph ignores
/// (9.6.4, Table 111).
fn color_entry(key: &[u8]) -> bool {
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

/// Table 58 is exhaustive for standard ExtGState entries. Skip extension values
/// before resolution: an ignored producer key must not trigger object loading.
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
