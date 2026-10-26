//! Colour-space resolution into `pagura_color::ColorSpace` (8.6.4–8.6.6):
//! names through the `ColorSpace` resources and `Default` spaces, arrays of
//! every family, ICC profiles cached by object, Indexed lookups and tint
//! transforms.

mod icc;

use super::function;
use super::numbers::{array, entry};
use crate::repairs;
use pagura_color::{
    ColorSpace, Colorant, DeviceN, ExponentialFunction, Function, Indexed, Separation,
    cie::Calibrated,
};
use pagura_core::{Diagnostic, Dictionary, Error, Name, ObjectId, Result, Store, Value};
use pagura_document::Document;
use std::sync::Arc;

pub(crate) use icc::Profiles;

/// Parameters outside this magnitude have no meaningful colorimetry.
const CIE_RANGE: f64 = 1e6;

/// The DeviceN colorant budget of this implementation (Annex C, Table C.1).
const MAX_COLORANTS: usize = 32;

/// Resolves colour spaces for one interpretation, sharing the device spaces,
/// ICC profiles (by object) and the tint transform of `All` and `None`.
#[derive(Debug)]
pub(crate) struct Colors<'a> {
    document: &'a Document,
    profiles: Profiles<'a>,
    gray: Arc<ColorSpace>,
    rgb: Arc<ColorSpace>,
    cmyk: Arc<ColorSpace>,
    complement: Option<Arc<Function>>,
}

/// Whether `Default` spaces replace device names (8.6.5.6). They do at the
/// top of a resolution and never inside a default's own definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Defaults {
    Apply,
    Ignore,
}

impl<'a> Colors<'a> {
    pub(crate) fn new(document: &'a Document, store: Option<&'a Store>) -> Self {
        Self {
            document,
            profiles: Profiles::new(document, store),
            gray: Arc::new(ColorSpace::DeviceGray),
            rgb: Arc::new(ColorSpace::DeviceRgb),
            cmyk: Arc::new(ColorSpace::DeviceCmyk),
            complement: None,
        }
    }

    /// The document.
    pub(crate) fn document(&self) -> &'a Document {
        self.document
    }

    /// Plain DeviceGray, without a `DefaultGray` lookup.
    pub(crate) fn device_gray(&self) -> Arc<ColorSpace> {
        Arc::clone(&self.gray)
    }

    /// Plain DeviceRGB, without a `DefaultRGB` lookup.
    pub(crate) fn device_rgb(&self) -> Arc<ColorSpace> {
        Arc::clone(&self.rgb)
    }

    /// Plain DeviceCMYK, without a `DefaultCMYK` lookup.
    pub(crate) fn device_cmyk(&self) -> Arc<ColorSpace> {
        Arc::clone(&self.cmyk)
    }

    /// Embedded JPX ICC colour (7.4.9) uses the existing forward profile
    /// transforms. A later profile version selects the ordinary-channel
    /// device fallback. It does not resolve PDF `Default` colour resources.
    pub(crate) fn embedded_icc(
        &mut self,
        id: ObjectId,
        data: &[u8],
        components: u8,
    ) -> Result<Arc<ColorSpace>> {
        let alternate = match components {
            1 => self.device_gray(),
            3 => self.device_rgb(),
            4 => self.device_cmyk(),
            _ => {
                return Err(Error::unsupported(
                    crate::features::XOBJECT_JPX_COLOR,
                    "embedded JPX ICC component count is not implemented",
                ));
            }
        };
        let profile = self.profiles.embedded(id, usize::from(components), data)?;
        if profile.as_ref().is_some_and(|profile| profile.lab_input()) {
            return Err(Error::unsupported(
                crate::features::XOBJECT_JPX_COLOR,
                "embedded JPX Lab-input ICC range interpretation is not implemented",
            ));
        }
        Ok(Arc::new(ColorSpace::Icc(Arc::new(
            pagura_color::IccSpace {
                profile,
                alternate,
                components,
                range: [[0.0, 1.0]; 4],
                object: Some(id),
            },
        ))))
    }

    /// The space a name selects in `resources`, as `cs`, `g`, `rg` and `k`
    /// select it.
    ///
    /// # Errors
    ///
    /// As [`Self::resolve`].
    pub(crate) fn named(&mut self, resources: &Dictionary, name: &[u8]) -> Result<Arc<ColorSpace>> {
        self.resolve(resources, &Value::name(name))
    }

    /// The space `value` describes in `resources`.
    ///
    /// # Errors
    ///
    /// `Syntax` for malformed spaces and undefined resources, `Unsupported`
    /// for unknown families, `Limit` for budgets, and the errors of ICC
    /// profiles and tint transforms.
    pub(crate) fn resolve(
        &mut self,
        resources: &Dictionary,
        value: &Value,
    ) -> Result<Arc<ColorSpace>> {
        self.space(resources, value, 0, Defaults::Apply)
    }

    fn space(
        &mut self,
        resources: &Dictionary,
        value: &Value,
        depth: usize,
        defaults: Defaults,
    ) -> Result<Arc<ColorSpace>> {
        let document = self.document;
        if depth >= document.limits().depth {
            return Err(Error::limit("colour space nesting exceeded"));
        }
        match document.resolve(value)? {
            Value::Name(name) => self.name(resources, name, depth, defaults),
            Value::Array(values) => self.array(resources, values, depth, defaults),
            _ => Err(Error::syntax("invalid colour space")),
        }
    }

    fn name(
        &mut self,
        resources: &Dictionary,
        name: &Name,
        depth: usize,
        defaults: Defaults,
    ) -> Result<Arc<ColorSpace>> {
        let document = self.document;
        if name == b"Pattern" {
            return Ok(Arc::new(ColorSpace::Pattern(None)));
        }
        let (device, default) = match name.as_bytes() {
            b"DeviceGray" => (Some(&self.gray), b"DefaultGray".as_slice()),
            b"DeviceRGB" => (Some(&self.rgb), b"DefaultRGB".as_slice()),
            b"DeviceCMYK" => (Some(&self.cmyk), b"DefaultCMYK".as_slice()),
            _ => (None, b"".as_slice()),
        };
        let spaces = resources
            .get(b"ColorSpace")
            .map(|v| document.resolve_dictionary(v))
            .transpose()?;
        if let Some(device) = device {
            let device = Arc::clone(device);
            if defaults == Defaults::Apply
                && let Some(value) = spaces.and_then(|d| d.get(default))
            {
                let alternate = self.space(resources, value, depth + 1, Defaults::Ignore)?;
                let lab = matches!(alternate.as_ref(), ColorSpace::Calibrated(c) if c.is_lab());
                if lab
                    || matches!(
                        alternate.as_ref(),
                        ColorSpace::Pattern(_) | ColorSpace::Indexed(_)
                    )
                {
                    return Err(Error::syntax("invalid default colour space family"));
                }
                if alternate.components() != device.components() {
                    return Err(Error::syntax(
                        "default colour space has wrong component count",
                    ));
                }
                return Ok(alternate);
            }
            return Ok(device);
        }
        if depth != 0 {
            return Err(Error::syntax(
                "nested colour space requires an explicit definition",
            ));
        }
        let value = spaces
            .and_then(|d| d.get(name.as_bytes()))
            .ok_or(Error::syntax("undefined colour space resource"))?;
        self.space(resources, value, depth + 1, defaults)
    }

    fn array(
        &mut self,
        resources: &Dictionary,
        values: &[Value],
        depth: usize,
        defaults: Defaults,
    ) -> Result<Arc<ColorSpace>> {
        let family = values
            .first()
            .and_then(Value::as_name)
            .ok_or(Error::syntax("colour space array has no family"))?;
        match (family.as_bytes(), values) {
            (b"DeviceGray" | b"DeviceRGB" | b"DeviceCMYK", [name]) => {
                self.space(resources, name, depth, defaults)
            }
            (b"Pattern", [_]) => Ok(Arc::new(ColorSpace::Pattern(None))),
            (b"Pattern", [_, base]) => {
                let base = self.space(resources, base, depth + 1, defaults)?;
                if matches!(base.as_ref(), ColorSpace::Pattern(_)) {
                    return Err(Error::syntax("nested Pattern colour space"));
                }
                Ok(Arc::new(ColorSpace::Pattern(Some(base))))
            }
            (b"Separation", [_, name, base, tint]) => {
                self.separation(resources, [name, base, tint], depth, defaults)
            }
            (b"DeviceN", [_, names, base, tint] | [_, names, base, tint, _]) => {
                self.device_n(resources, values, [names, base, tint], (depth, defaults))
            }
            (b"ICCBased", [_, profile]) => self.icc(resources, profile, depth),
            (b"CalGray" | b"CalRGB" | b"Lab", [_, parameters]) => {
                let parameters = self.document.resolve_dictionary(parameters)?;
                Ok(Arc::new(ColorSpace::Calibrated(Arc::new(calibrated(
                    self.document,
                    family.as_bytes(),
                    parameters,
                )?))))
            }
            (b"Indexed", [_, base, high, lookup]) => {
                self.indexed(resources, [base, high, lookup], depth, defaults)
            }
            (
                b"Pattern" | b"ICCBased" | b"Indexed" | b"CalGray" | b"CalRGB" | b"Lab"
                | b"Separation" | b"DeviceN",
                _,
            ) => Err(Error::syntax("wrong colour space array length")),
            _ => Err(Error::unsupported(
                pagura_color::features::UNKNOWN_FAMILY,
                "colour space family is not implemented",
            )),
        }
    }

    fn separation(
        &mut self,
        resources: &Dictionary,
        [name, base, tint]: [&Value; 3],
        depth: usize,
        defaults: Defaults,
    ) -> Result<Arc<ColorSpace>> {
        let name = self
            .document
            .resolve(name)?
            .as_name()
            .ok_or(Error::syntax("Separation requires a colourant name"))?;
        // `All` and `None` select no alternate (8.6.6.4): neither the
        // alternate space nor the tint transform is read.
        let colorant = match name.as_bytes() {
            b"All" => Some(Colorant::All),
            b"None" => Some(Colorant::None),
            _ => None,
        };
        if let Some(colorant) = colorant {
            return Ok(Arc::new(ColorSpace::Separation(Arc::new(Separation {
                colorant,
                alternate: self.device_gray(),
                tint: self.complement()?,
            }))));
        }
        let (alternate, tint) = self.tint(resources, base, tint, 1, (depth, defaults))?;
        Ok(Arc::new(ColorSpace::Separation(Arc::new(Separation {
            colorant: Colorant::Named(name.clone()),
            alternate,
            tint,
        }))))
    }

    fn device_n(
        &mut self,
        resources: &Dictionary,
        values: &[Value],
        [names, base, tint]: [&Value; 3],
        (depth, defaults): (usize, Defaults),
    ) -> Result<Arc<ColorSpace>> {
        let document = self.document;
        let attributes = values
            .get(4)
            .map(|v| document.resolve_dictionary(v))
            .transpose()?;
        let colorants = self.colorants(names)?;
        if let Some(attributes) = attributes
            && entry(document, attributes, b"Subtype")?
                .and_then(Value::as_name)
                .is_some_and(|n| n == b"NChannel")
            && colorants.contains(&Colorant::None)
        {
            return Err(Error::syntax("None is forbidden in NChannel"));
        }
        if colorants.iter().all(|c| *c == Colorant::None) {
            return Ok(Arc::new(ColorSpace::DeviceN(Arc::new(DeviceN {
                colorants,
                alternate: self.device_gray(),
                tint: self.complement()?,
            }))));
        }
        let (alternate, tint) =
            self.tint(resources, base, tint, colorants.len(), (depth, defaults))?;
        Ok(Arc::new(ColorSpace::DeviceN(Arc::new(DeviceN {
            colorants,
            alternate,
            tint,
        }))))
    }

    /// DeviceN colorant names: at most 32, no `All`, no repeated name other
    /// than `None`.
    fn colorants(&self, names: &Value) -> Result<Vec<Colorant>> {
        let document = self.document;
        let names = document
            .resolve(names)?
            .as_array()
            .filter(|n| !n.is_empty())
            .ok_or(Error::syntax("DeviceN requires colourant names"))?;
        if names.len() > MAX_COLORANTS {
            return Err(Error::limit(
                "DeviceN component count exceeds implementation limit (Annex C)",
            ));
        }
        let mut colorants = Vec::with_capacity(names.len());
        for name in names {
            let name = document
                .resolve(name)?
                .as_name()
                .ok_or(Error::syntax("DeviceN colourant must be a name"))?;
            if name == b"All" {
                return Err(Error::syntax("All is forbidden in DeviceN"));
            }
            let colorant = if name == b"None" {
                Colorant::None
            } else {
                Colorant::Named(name.clone())
            };
            if colorant != Colorant::None && colorants.contains(&colorant) {
                return Err(Error::syntax("duplicate DeviceN colourant"));
            }
            colorants.push(colorant);
        }
        Ok(colorants)
    }

    /// The alternate space and tint transform of a Separation or DeviceN
    /// space with `inputs` colorants.
    fn tint(
        &mut self,
        resources: &Dictionary,
        base: &Value,
        tint: &Value,
        inputs: usize,
        (depth, defaults): (usize, Defaults),
    ) -> Result<(Arc<ColorSpace>, Arc<Function>)> {
        let base = self.space(resources, base, depth + 1, defaults)?;
        if matches!(
            base.as_ref(),
            ColorSpace::Separation(_)
                | ColorSpace::DeviceN(_)
                | ColorSpace::Indexed(_)
                | ColorSpace::Pattern(_)
        ) {
            return Err(Error::syntax("invalid tint alternate space"));
        }
        let function = function::read(self.document, tint, inputs, base.components())?;
        Ok((base, function.function))
    }

    /// The tint transform of `All` and `None` spaces: one minus the tint into
    /// DeviceGray. `None` paints nothing, so its transform is never applied.
    fn complement(&mut self) -> Result<Arc<Function>> {
        if let Some(function) = &self.complement {
            return Ok(Arc::clone(function));
        }
        let function = Arc::new(Function::exponential(ExponentialFunction {
            domain: [0.0, 1.0],
            range: None,
            c0: vec![1.0],
            c1: vec![0.0],
            exponent: 1.0,
        })?);
        self.complement = Some(Arc::clone(&function));
        Ok(function)
    }

    fn indexed(
        &mut self,
        resources: &Dictionary,
        [base, high, lookup]: [&Value; 3],
        depth: usize,
        defaults: Defaults,
    ) -> Result<Arc<ColorSpace>> {
        let document = self.document;
        let base = self.space(resources, base, depth + 1, defaults)?;
        if matches!(
            base.as_ref(),
            ColorSpace::Indexed(_) | ColorSpace::Pattern(_)
        ) {
            return Err(Error::syntax("Indexed base cannot be Indexed"));
        }
        let high = document
            .resolve(high)?
            .as_integer()
            .and_then(|n| u8::try_from(n).ok())
            .ok_or(Error::syntax("invalid Indexed high value"))?;
        let lookup_id = lookup.as_reference();
        let mut table = if let Some(id) = lookup_id {
            let object = document.object(id)?;
            if object.is_stream() {
                document.stream_data(object)?.into_owned()
            } else {
                document
                    .resolve(lookup)?
                    .as_string()
                    .ok_or(Error::syntax("invalid Indexed lookup"))?
                    .to_vec()
            }
        } else {
            lookup
                .as_string()
                .ok_or(Error::syntax("invalid Indexed lookup"))?
                .to_vec()
        };
        let required = (usize::from(high) + 1) * base.components();
        if table.len() < required {
            return Err(Error::syntax("Indexed lookup has wrong length"));
        }
        if table.len() > required {
            document.repair(
                Diagnostic::new(repairs::INDEXED_EXCESS, "Indexed lookup has wrong length")
                    .in_object_opt(lookup_id),
            )?;
            table.truncate(required);
        }
        Ok(Arc::new(ColorSpace::Indexed(Arc::new(Indexed {
            base,
            high,
            lookup: table.into(),
        }))))
    }

    fn icc(
        &mut self,
        resources: &Dictionary,
        profile: &Value,
        depth: usize,
    ) -> Result<Arc<ColorSpace>> {
        let read = icc::read(self.document, &mut self.profiles, profile)?;
        let default = match read.components {
            1 => self.device_gray(),
            3 => self.device_rgb(),
            _ => self.device_cmyk(),
        };
        // Validate Alternate independently of whether its rendering model is
        // used: a supported profile must not require support for an unused
        // alternate.
        let mut alternate = None;
        if let Some(value) = read.dictionary.get(b"Alternate") {
            if icc::alternate_components(self.document, resources, value, depth + 1)?
                != usize::from(read.components)
            {
                return Err(Error::syntax("ICC alternate has wrong component count"));
            }
            match self.space(resources, value, depth + 1, Defaults::Ignore) {
                Ok(space) => alternate = Some(space),
                Err(error) if read.profile.is_none() || !error.is_unsupported() => {
                    return Err(error);
                }
                Err(_) => {}
            }
        }
        icc::check_range(read.profile.as_deref(), &read.range, read.components)?;
        Ok(Arc::new(ColorSpace::Icc(Arc::new(
            pagura_color::IccSpace {
                profile: read.profile,
                alternate: alternate.unwrap_or(default),
                components: read.components,
                range: read.range,
                object: Some(read.object),
            },
        ))))
    }
}

/// Reads a `CalGray`, `CalRGB` or `Lab` parameter dictionary (8.6.5.2–8.6.5.4).
fn calibrated(document: &Document, family: &[u8], d: &Dictionary) -> Result<Calibrated> {
    let white = parameter::<3>(document, d, b"WhitePoint", None)?;
    let black = parameter::<3>(document, d, b"BlackPoint", Some([0.0; 3]))?;
    match family {
        b"CalGray" => {
            let gamma = d
                .get(b"Gamma")
                .map(|v| {
                    document
                        .resolve(v)?
                        .as_number()
                        .ok_or(Error::syntax("CalGray Gamma must be numeric"))
                })
                .transpose()?
                .unwrap_or(1.0);
            Calibrated::gray(white, black, gamma)
        }
        b"CalRGB" => {
            let gamma = parameter::<3>(document, d, b"Gamma", Some([1.0; 3]))?;
            // Matrix is column-major: XA YA ZA XB YB ZB XC YC ZC (8.6.5.3).
            let m = parameter::<9>(
                document,
                d,
                b"Matrix",
                Some([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]),
            )?;
            let matrix = [[m[0], m[3], m[6]], [m[1], m[4], m[7]], [m[2], m[5], m[8]]];
            Calibrated::rgb(white, black, gamma, matrix)
        }
        _ => {
            let range =
                parameter::<4>(document, d, b"Range", Some([-100.0, 100.0, -100.0, 100.0]))?;
            Calibrated::lab(white, black, range)
        }
    }
}

fn parameter<const N: usize>(
    document: &Document,
    d: &Dictionary,
    key: &[u8],
    default: Option<[f64; N]>,
) -> Result<[f64; N]> {
    let Some(value) = d.get(key) else {
        return default.ok_or(Error::syntax("missing CIE parameter"));
    };
    array(document, value, CIE_RANGE)
}

#[cfg(test)]
mod tests;
