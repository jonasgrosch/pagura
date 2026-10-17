//! Source-space colours and device conversions (8.6 and 10.4).

use crate::features;
use pagura_core::{Dictionary, Error, Feature, Result, Value};
use pagura_document::Document;
use pagura_raster::Rgba8;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub(crate) enum Space {
    Pattern(Option<Arc<Self>>),
    Gray,
    Rgb,
    Cmyk,
    Cie(Arc<crate::cie::Cie>),
    Tint {
        inputs: usize,
        base: Arc<Self>,
        function: Arc<crate::functions::Function>,
    },
    Indexed {
        base: Arc<Self>,
        high: u8,
        lookup: Arc<[u8]>,
    },
}

#[derive(Debug, Clone)]
pub(crate) enum Color {
    Solid {
        space: Space,
        components: Vec<f64>,
    },
    Pattern {
        base: Option<Arc<Space>>,
        selected: Option<Arc<crate::pattern::Pattern>>,
    },
}
impl Default for Color {
    fn default() -> Self {
        Self::new(Space::Gray)
    }
}
impl Color {
    pub(crate) fn new(space: Space) -> Self {
        if let Space::Pattern(base) = space {
            return Self::Pattern {
                base,
                selected: None,
            };
        }
        let components = match &space {
            Space::Cmyk => vec![0.0, 0.0, 0.0, 1.0],
            Space::Tint { inputs, .. } => vec![1.0; *inputs],
            _ => vec![0.0; space.components()],
        };
        Self::Solid { space, components }
    }
    pub(crate) fn set(&mut self, values: &[Value]) -> Result<()> {
        let Self::Solid { space, components } = self else {
            return Err(Error::syntax("pattern requires SCN or scn"));
        };
        if values.len() != space.components() {
            return Err(Error::syntax("wrong colour component count"));
        }
        *components = values
            .iter()
            .map(|v| {
                v.as_number()
                    .ok_or(Error::syntax("colour component must be a number"))
            })
            .collect::<Result<_>>()?;
        Ok(())
    }
    pub(crate) fn select_pattern(
        &mut self,
        document: &Document,
        resources: &Dictionary,
        values: &[Value],
        initial: pagura_raster::Transform,
    ) -> Result<()> {
        let Self::Pattern { base, selected } = self else {
            return self.set(values);
        };
        let Some((name, components)) = values.split_last() else {
            return Err(Error::syntax("missing pattern name"));
        };
        let name = name
            .as_name()
            .ok_or(Error::syntax("pattern name expected"))?;
        let pattern =
            crate::pattern::Pattern::parse(document, resources, name.as_bytes(), initial)?;
        if base.is_some() || !components.is_empty() {
            return Err(Error::syntax(
                "shading pattern cannot have an underlying colour",
            ));
        }
        *selected = Some(Arc::new(pattern));
        Ok(())
    }
    pub(crate) fn to_rgba(&self, transfer: Option<&crate::functions::Transfer>) -> Result<Rgba8> {
        let Self::Solid { space, components } = self else {
            return Err(Error::invalid_argument("pattern is not a uniform colour"));
        };
        let mut rgb = space.convert(components)?;
        if let Some(transfer) = transfer {
            for (channel, function) in rgb.iter_mut().zip(transfer.iter()) {
                *channel = function
                    .evaluate(&[channel.clamp(0.0, 1.0)])?
                    .first()
                    .copied()
                    .ok_or(Error::syntax("transfer function has no output"))?;
            }
        }
        let [r, g, b] = rgb.map(channel);
        Ok(Rgba8::opaque(r, g, b))
    }
}

impl Space {
    pub(crate) fn components(&self) -> usize {
        match self {
            Self::Pattern(base) => base.as_ref().map_or(0, |b| b.components()),
            Self::Gray | Self::Indexed { .. } => 1,
            Self::Rgb => 3,
            Self::Cmyk => 4,
            Self::Cie(cie) => cie.components(),
            Self::Tint { inputs, .. } => *inputs,
        }
    }

    pub(crate) fn convert(&self, c: &[f64]) -> Result<[f64; 3]> {
        let n = |i: usize| c.get(i).copied().unwrap_or(0.0).clamp(0.0, 1.0);
        match self {
            Self::Pattern(_) => Err(Error::invalid_argument(
                "pattern is not a component colour space",
            )),
            Self::Gray => Ok([n(0); 3]),
            Self::Rgb => Ok([n(0), n(1), n(2)]),
            Self::Cmyk => {
                let k = n(3);
                Ok([
                    1.0 - (n(0) + k).min(1.0),
                    1.0 - (n(1) + k).min(1.0),
                    1.0 - (n(2) + k).min(1.0),
                ])
            }
            Self::Cie(cie) => Ok(cie.convert(c)),
            Self::Tint { base, function, .. } => {
                let tint: Vec<_> = c.iter().map(|v| v.clamp(0.0, 1.0)).collect();
                base.convert(&function.evaluate(&tint)?)
            }
            Self::Indexed { base, high, lookup } => {
                let value = c
                    .first()
                    .copied()
                    .unwrap_or(0.0)
                    .round()
                    .clamp(0.0, f64::from(*high));
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "index is clamped to 0..=255"
                )]
                let index = value as usize;
                let bytes = lookup
                    .get(index * base.components()..(index + 1) * base.components())
                    .unwrap_or_default();
                let components: Vec<_> = if let Self::Cie(cie) = base.as_ref() {
                    cie.decode_lookup(bytes)
                } else {
                    bytes.iter().map(|&b| f64::from(b) / 255.0).collect()
                };
                base.convert(&components)
            }
        }
    }
}

pub(crate) fn named(document: &Document, resources: &Dictionary, name: &[u8]) -> Result<Space> {
    resolve(document, resources, &Value::name(name), 0, true)
}

pub(crate) fn blending(
    document: &Document,
    resources: &Dictionary,
    value: &Value,
) -> Result<Space> {
    resolve(document, resources, value, 0, true)
}

fn resolve(
    document: &Document,
    resources: &Dictionary,
    value: &Value,
    depth: usize,
    defaults: bool,
) -> Result<Space> {
    if depth >= document.limits().depth {
        return Err(Error::limit("colour space nesting exceeded"));
    }
    match document.resolve(value)? {
        Value::Name(name) => {
            if name == b"Pattern" {
                return Ok(Space::Pattern(None));
            }
            let (device, default) = match name.as_bytes() {
                b"DeviceGray" => (Some(Space::Gray), b"DefaultGray".as_slice()),
                b"DeviceRGB" => (Some(Space::Rgb), b"DefaultRGB".as_slice()),
                b"DeviceCMYK" => (Some(Space::Cmyk), b"DefaultCMYK".as_slice()),
                _ => (None, b"".as_slice()),
            };
            let spaces = resources
                .get(b"ColorSpace")
                .map(|v| document.resolve_dictionary(v))
                .transpose()?;
            if let Some(device) = device {
                if defaults && let Some(value) = spaces.and_then(|d| d.get(default)) {
                    let alternate = resolve(document, resources, value, depth + 1, false)?;
                    if matches!(alternate, Space::Pattern(_))
                        || alternate.components() != device.components()
                    {
                        return Err(Error::syntax(
                            "default colour space has wrong component count",
                        ));
                    }
                    return Ok(alternate);
                }
                return Ok(device);
            }
            if let Some(feature) = unsupported(name.as_bytes()) {
                return Err(Error::unsupported(
                    feature,
                    "colour space is not implemented",
                ));
            }
            let value = spaces
                .and_then(|d| d.get(name.as_bytes()))
                .ok_or(Error::syntax("undefined colour space resource"))?;
            resolve(document, resources, value, depth + 1, defaults)
        }
        Value::Array(values) => array_space(document, resources, values, depth),
        _ => Err(Error::syntax("invalid colour space")),
    }
}

fn array_space(
    document: &Document,
    resources: &Dictionary,
    values: &[Value],
    depth: usize,
) -> Result<Space> {
    let family = values
        .first()
        .and_then(Value::as_name)
        .ok_or(Error::syntax("colour space array has no family"))?;
    match (family.as_bytes(), values) {
        (b"Pattern", [_, base]) => {
            let base = resolve(document, resources, base, depth + 1, false)?;
            if matches!(base, Space::Pattern(_)) {
                return Err(Error::syntax("nested Pattern colour space"));
            }
            Ok(Space::Pattern(Some(Arc::new(base))))
        }

        (b"Separation", [_, name, base, function]) => {
            let name = document
                .resolve(name)?
                .as_name()
                .ok_or(Error::syntax("Separation requires a colourant name"))?;
            if matches!(name.as_bytes(), b"None" | b"All") {
                return Err(Error::unsupported(
                    features::COLORSPACE_SPECIAL_INK,
                    "special colourant semantics are not implemented",
                ));
            }
            tint_space(document, resources, base, function, 1, depth)
        }
        (b"DeviceN", [_, names, base, function] | [_, names, base, function, _]) => {
            if let Some(attributes) = values.get(4) {
                document.resolve_dictionary(attributes)?;
            }
            let count = device_n_components(document, names)?;
            tint_space(document, resources, base, function, count, depth)
        }
        (b"ICCBased", [_, profile]) => icc_space(document, resources, profile, depth),
        (b"CalGray" | b"CalRGB" | b"Lab", [_, parameters]) => {
            let parameters = document.resolve_dictionary(parameters)?;
            Ok(Space::Cie(Arc::new(crate::cie::Cie::parse(
                document,
                family.as_bytes(),
                parameters,
            )?)))
        }
        (b"Indexed", [_, base, high, lookup]) => {
            let base = resolve(document, resources, base, depth + 1, false)?;
            if matches!(base, Space::Indexed { .. } | Space::Pattern(_)) {
                return Err(Error::syntax("Indexed base cannot be Indexed"));
            }
            let high = document
                .resolve(high)?
                .as_integer()
                .and_then(|n| u8::try_from(n).ok())
                .ok_or(Error::syntax("invalid Indexed high value"))?;
            let lookup = if let Some(id) = lookup.as_reference() {
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
            if lookup.len() < required {
                return Err(Error::syntax("Indexed lookup is too short"));
            }
            Ok(Space::Indexed {
                base: Arc::new(base),
                high,
                lookup: lookup.into(),
            })
        }
        (
            b"ICCBased" | b"Indexed" | b"CalGray" | b"CalRGB" | b"Lab" | b"Separation" | b"DeviceN",
            _,
        ) => Err(Error::syntax("wrong colour space array length")),
        _ => Err(Error::unsupported(
            unsupported(family.as_bytes()).unwrap_or(features::COLORSPACE_UNKNOWN),
            "colour space family is not implemented",
        )),
    }
}

fn device_n_components(document: &Document, names: &Value) -> Result<usize> {
    let names = document
        .resolve(names)?
        .as_array()
        .filter(|n| !n.is_empty())
        .ok_or(Error::syntax("DeviceN requires colourant names"))?;
    if names.len() > 32 {
        return Err(Error::limit("DeviceN component count exceeds PDF limit"));
    }
    let mut seen = std::collections::BTreeSet::new();
    for name in names {
        let name = document
            .resolve(name)?
            .as_name()
            .ok_or(Error::syntax("DeviceN colourant must be a name"))?;
        if !seen.insert(name.as_bytes()) {
            return Err(Error::syntax("duplicate DeviceN colourant"));
        }
        if matches!(name.as_bytes(), b"None" | b"All") {
            return Err(Error::unsupported(
                features::COLORSPACE_SPECIAL_INK,
                "special colourant semantics are not implemented",
            ));
        }
    }
    Ok(names.len())
}

fn tint_space(
    document: &Document,
    resources: &Dictionary,
    base: &Value,
    function: &Value,
    inputs: usize,
    depth: usize,
) -> Result<Space> {
    let base = resolve(document, resources, base, depth + 1, false)?;
    if matches!(
        base,
        Space::Indexed { .. } | Space::Tint { .. } | Space::Pattern(_)
    ) {
        return Err(Error::syntax("invalid tint alternate space"));
    }
    let function =
        crate::functions::Function::parse(document, function, inputs, base.components())?;
    Ok(Space::Tint {
        inputs,
        base: Arc::new(base),
        function: Arc::new(function),
    })
}

fn icc_space(
    document: &Document,
    resources: &Dictionary,
    profile: &Value,
    depth: usize,
) -> Result<Space> {
    let id = profile
        .as_reference()
        .ok_or(Error::syntax("ICC profile must be a stream reference"))?;
    let object = document.object(id)?;
    if !object.is_stream() {
        return Err(Error::syntax("ICC profile must be a stream"));
    }
    let dictionary = object
        .dictionary()
        .ok_or(Error::syntax("ICC stream has no dictionary"))?;
    let count = dictionary
        .get(b"N")
        .map(|v| document.resolve(v))
        .transpose()?
        .and_then(Value::as_integer)
        .ok_or(Error::syntax("ICC profile has no N"))?;
    let default = match count {
        1 => Space::Gray,
        3 => Space::Rgb,
        4 => Space::Cmyk,
        _ => return Err(Error::syntax("invalid ICC component count")),
    };
    if let Some(range) = dictionary.get(b"Range") {
        let range = document
            .resolve(range)?
            .as_array()
            .ok_or(Error::syntax("ICC Range must be an array"))?;
        let expected =
            usize::try_from(count).map_err(|_| Error::syntax("invalid ICC component count"))? * 2;
        if range.len() != expected {
            return Err(Error::syntax("wrong ICC Range length"));
        }
        for pair in range.as_chunks::<2>().0 {
            let low = document
                .resolve(&pair[0])?
                .as_number()
                .ok_or(Error::syntax("invalid ICC Range bound"))?;
            let high = document
                .resolve(&pair[1])?
                .as_number()
                .ok_or(Error::syntax("invalid ICC Range bound"))?;
            if low > high {
                return Err(Error::syntax("reversed ICC Range"));
            }
            if low != 0.0 || high != 1.0 {
                return Err(Error::unsupported(
                    features::COLORSPACE_ICC_RANGE,
                    "nondefault ICC component ranges are not implemented",
                ));
            }
        }
    }
    // 8.6.5.5 permits Alternate when the profile is unsupported.
    let alternate = dictionary
        .get(b"Alternate")
        .map(|v| resolve(document, resources, v, depth + 1, false))
        .transpose()?
        .unwrap_or(default);
    if alternate.components() != usize::try_from(count).unwrap_or(0) {
        return Err(Error::syntax("ICC alternate has wrong component count"));
    }
    Ok(alternate)
}

fn unsupported(name: &[u8]) -> Option<Feature> {
    Some(match name {
        b"Separation" => features::COLORSPACE_SEPARATION,
        b"DeviceN" => features::COLORSPACE_DEVICEN,
        b"Lab" => features::COLORSPACE_LAB,
        b"CalGray" => features::COLORSPACE_CALGRAY,
        b"CalRGB" => features::COLORSPACE_CALRGB,
        b"Pattern" => features::PATTERN_COLORSPACE,
        _ => return None,
    })
}

/// Quantizes a colour component in `0..=1` to an 8-bit channel, rounding to
/// nearest; out-of-range components are clamped first (clause 8.6.4.1).
pub(crate) fn channel(value: f64) -> u8 {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to 0..=255 before conversion"
    )]
    let channel = (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    channel
}
