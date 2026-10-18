//! Source-space colours and device conversions (8.6 and 10.4).

use crate::features;
use crate::icc::ColorContext;
use pagura_core::{Dictionary, Error, Feature, Result, Value};
use pagura_raster::Rgba8;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub(crate) enum Space {
    Pattern(Option<Arc<Self>>),
    Special {
        inputs: usize,
        all: bool,
    },
    Gray,
    Rgb,
    Cmyk,
    Cie(Arc<crate::cie::Cie>),
    Icc {
        base: Arc<Self>,
        profile: Option<Arc<crate::icc::Profile>>,
        range: [[f64; 2]; 4],
    },
    Tint {
        names: Arc<[pagura_core::Name]>,
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
    pub(crate) fn paints(&self) -> bool {
        match self {
            Self::Solid { space, .. } => space.paints(),
            Self::Pattern { .. } => true,
        }
    }
    pub(crate) fn new(space: Space) -> Self {
        if let Space::Pattern(base) = space {
            return Self::Pattern {
                base,
                selected: None,
            };
        }
        let components = match &space {
            Space::Cmyk => vec![0.0, 0.0, 0.0, 1.0],
            Space::Tint { names, .. } => vec![1.0; names.len()],
            Space::Special { inputs, .. } => vec![1.0; *inputs],
            _ => vec![0.0; space.components()],
        };
        Self::Solid { space, components }
    }
    pub(crate) fn validate_simple_operator(
        &self,
        document: &pagura_document::Document,
        values: &[Value],
        position: pagura_core::Position,
    ) -> Result<()> {
        // Table 73: SC/sc do not select special tint or ICCBased colours.
        if let Self::Solid {
            space: space @ (Space::Tint { .. } | Space::Special { .. } | Space::Icc { .. }),
            ..
        } = self
        {
            if values.len() != space.components() || values.iter().any(|v| v.as_number().is_none())
            {
                return Err(Error::syntax("colour space requires SCN or scn"));
            }
            let mut diagnostic = pagura_core::Diagnostic::new(
                crate::interpreter::tags::COLOUR_OPERATOR,
                "colour space requires SCN or scn",
            )
            .at(position);
            if let pagura_core::Position::Stream { object, .. } = position {
                diagnostic = diagnostic.in_object(object);
            }
            crate::interpreter::repair(document, diagnostic)?;
        }
        Ok(())
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
        document: &ColorContext<'_>,
        resources: &Dictionary,
        values: &[Value],
        initial: pagura_raster::Transform,
        position: pagura_core::Position,
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
        let mut pattern =
            crate::pattern::Pattern::parse(document, resources, name.as_bytes(), initial)?;
        match &mut pattern.kind {
            crate::pattern::Kind::Tiling(t) if t.uncolored => {
                let space = base.as_ref().ok_or(Error::syntax(
                    "uncoloured pattern requires underlying space",
                ))?;
                let mut color = Color::new(space.as_ref().clone());
                color.set(components)?;
                t.color = Some(color);
            }
            _ if !components.is_empty() => {
                if components.iter().any(|v| v.as_number().is_none())
                    || base
                        .as_ref()
                        .is_none_or(|s| s.components() != components.len())
                {
                    return Err(Error::syntax("invalid unused pattern colour operands"));
                }
                let mut diagnostic = pagura_core::Diagnostic::new(
                    crate::interpreter::tags::PATTERN_UNDERLYING,
                    "coloured pattern uses intrinsic colours; extra scn operands ignored (8.7.3.2, Table 73)",
                ).at(position);
                if let pagura_core::Position::Stream { object, .. } = position {
                    diagnostic = diagnostic.in_object(object);
                }
                crate::interpreter::repair(document, diagnostic)?;
            }
            _ => {}
        }
        *selected = Some(Arc::new(pattern));
        Ok(())
    }
    pub(crate) fn to_rgba(
        &self,
        transfer: Option<&crate::functions::Transfer>,
        intent: crate::icc::Intent,
    ) -> Result<Rgba8> {
        let Self::Solid { space, components } = self else {
            return Err(Error::invalid_argument("pattern is not a uniform colour"));
        };
        if !space.paints() {
            return Ok(Rgba8::TRANSPARENT);
        }
        let mut rgb = space.convert_with_intent(components, intent)?;
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
    /// PDF Table 65 constrains source values before any Alternate substitution.
    pub(crate) fn clip_icc(c: &[f64], range: &[[f64; 2]; 4]) -> [f64; 4] {
        [0, 1, 2, 3].map(|i| {
            c.get(i)
                .copied()
                .unwrap_or(0.0)
                .clamp(range[i][0], range[i][1])
        })
    }

    /// Map Indexed lookup bytes to the base space's source range (8.6.6.3).
    pub(crate) fn decode_lookup(&self, bytes: &[u8]) -> Vec<f64> {
        match self {
            Self::Cie(cie) => cie.decode_lookup(bytes),
            Self::Icc { range, .. } => bytes
                .iter()
                .zip(range)
                .map(|(&v, &[low, high])| {
                    low * (1.0 - f64::from(v) / 255.0) + high * (f64::from(v) / 255.0)
                })
                .collect(),
            _ => bytes.iter().map(|&v| f64::from(v) / 255.0).collect(),
        }
    }

    /// None contributes neither colour, opacity nor shape (8.6.6.4–8.6.6.5).
    pub(crate) fn paints(&self) -> bool {
        match self {
            Self::Special { all: false, .. } => false,
            Self::Indexed { base, .. }
            | Self::Icc {
                base,
                profile: None,
                ..
            } => base.paints(),
            _ => true,
        }
    }

    pub(crate) fn blending_fallback(self) -> Self {
        if let Self::Icc {
            base,
            profile: None,
            ..
        } = &self
        {
            return base.as_ref().clone();
        }
        self
    }
    pub(crate) fn components(&self) -> usize {
        match self {
            Self::Pattern(base) => base.as_ref().map_or(0, |b| b.components()),
            Self::Gray | Self::Indexed { .. } => 1,
            Self::Rgb => 3,
            Self::Cmyk => 4,
            Self::Cie(cie) => cie.components(),
            Self::Icc { base, .. } => base.components(),
            Self::Tint { names, .. } => names.len(),
            Self::Special { inputs, .. } => *inputs,
        }
    }

    pub(crate) fn convert_with_intent(
        &self,
        c: &[f64],
        intent: crate::icc::Intent,
    ) -> Result<[f64; 3]> {
        let n = |i: usize| c.get(i).copied().unwrap_or(0.0).clamp(0.0, 1.0);
        match self {
            Self::Pattern(_) => Err(Error::invalid_argument(
                "pattern is not a component colour space",
            )),
            Self::Special { .. } => Ok([1.0 - n(0); 3]),
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
            Self::Icc {
                base,
                profile,
                range,
            } => {
                let clipped = Self::clip_icc(c, range);
                let clipped = &clipped[..base.components()];
                profile.as_ref().map_or_else(
                    || base.convert_with_intent(clipped, intent),
                    |p| p.convert(clipped, intent),
                )
            }
            Self::Tint { base, function, .. } => {
                let tint: Vec<_> = c.iter().map(|v| v.clamp(0.0, 1.0)).collect();
                base.convert_with_intent(&function.evaluate(&tint)?, intent)
            }
            Self::Indexed { base, high, lookup } => {
                let components =
                    indexed_components(base, *high, lookup, c.first().copied().unwrap_or(0.0))?;
                base.convert_with_intent(&components, intent)
            }
        }
    }
}

/// Decode one Indexed entry in its base space's source range (8.6.6.3).
pub(crate) fn indexed_components(
    base: &Space,
    high: u8,
    lookup: &[u8],
    value: f64,
) -> Result<Vec<f64>> {
    let value = value.round().clamp(0.0, f64::from(high));
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "index rounded and clamped to 0..255"
    )]
    let index = value as usize;
    let count = base.components();
    let bytes = lookup
        .get(index * count..(index + 1) * count)
        .ok_or(Error::syntax("truncated Indexed colour lookup"))?;
    Ok(base.decode_lookup(bytes))
}

pub(crate) fn named(
    document: &ColorContext<'_>,
    resources: &Dictionary,
    name: &[u8],
) -> Result<Space> {
    resolve(document, resources, &Value::name(name), 0, true)
}

pub(crate) fn blending(
    document: &ColorContext<'_>,
    resources: &Dictionary,
    value: &Value,
) -> Result<Space> {
    resolve(document, resources, value, 0, true)
}

fn resolve(
    document: &ColorContext<'_>,
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
                    if matches!(alternate, Space::Pattern(_) | Space::Indexed { .. })
                        || matches!(&alternate, Space::Cie(cie) if cie.is_lab())
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
            if let Some(feature) = unsupported(name.as_bytes()) {
                return Err(Error::unsupported(
                    feature,
                    "colour space is not implemented",
                ));
            }
            if depth != 0 {
                return Err(Error::syntax(
                    "nested colour space requires an explicit definition",
                ));
            }
            let value = spaces
                .and_then(|d| d.get(name.as_bytes()))
                .ok_or(Error::syntax("undefined colour space resource"))?;
            resolve(document, resources, value, depth + 1, defaults)
        }
        Value::Array(values) => array_space(document, resources, values, depth, defaults),
        _ => Err(Error::syntax("invalid colour space")),
    }
}

fn array_space(
    document: &ColorContext<'_>,
    resources: &Dictionary,
    values: &[Value],
    depth: usize,
    defaults: bool,
) -> Result<Space> {
    let family = values
        .first()
        .and_then(Value::as_name)
        .ok_or(Error::syntax("colour space array has no family"))?;
    match (family.as_bytes(), values) {
        (b"DeviceGray" | b"DeviceRGB" | b"DeviceCMYK", [_]) => {
            resolve(document, resources, &values[0], depth, defaults)
        }
        (b"Pattern", [_]) => Ok(Space::Pattern(None)),
        (b"Pattern", [_, base]) => {
            let base = resolve(document, resources, base, depth + 1, defaults)?;
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
                return Ok(Space::Special {
                    inputs: 1,
                    all: name == b"All",
                });
            }
            tint_space(
                document,
                resources,
                base,
                function,
                vec![name.clone()],
                (depth, defaults),
            )
        }
        (b"DeviceN", [_, names, base, function] | [_, names, base, function, _]) => device_n_space(
            document,
            resources,
            values,
            [names, base, function],
            (depth, defaults),
        ),
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
            indexed_space(document, resources, [base, high, lookup], depth, defaults)
        }

        (
            b"Pattern" | b"ICCBased" | b"Indexed" | b"CalGray" | b"CalRGB" | b"Lab" | b"Separation"
            | b"DeviceN",
            _,
        ) => Err(Error::syntax("wrong colour space array length")),
        _ => Err(Error::unsupported(
            unsupported(family.as_bytes()).unwrap_or(features::COLORSPACE_UNKNOWN),
            "colour space family is not implemented",
        )),
    }
}

fn device_n_space(
    document: &ColorContext<'_>,
    resources: &Dictionary,
    values: &[Value],
    [names, base, function]: [&Value; 3],
    (depth, defaults): (usize, bool),
) -> Result<Space> {
    let attributes = values
        .get(4)
        .map(|v| document.resolve_dictionary(v))
        .transpose()?;
    let count = device_n_components(document, names)?;
    if let Some(attributes) = attributes
        && attributes
            .get(b"Subtype")
            .map(|v| document.resolve(v))
            .transpose()?
            .and_then(Value::as_name)
            .is_some_and(|n| n == b"NChannel")
    {
        for name in document.resolve(names)?.as_array().unwrap_or_default() {
            if document
                .resolve(name)?
                .as_name()
                .is_some_and(|n| n == b"None")
            {
                return Err(Error::syntax("None is forbidden in NChannel"));
            }
        }
    }
    if document.resolve(names)?.as_array().is_some_and(|names| {
        names.iter().all(|n| {
            document
                .resolve(n)
                .ok()
                .and_then(Value::as_name)
                .is_some_and(|n| n == b"None")
        })
    }) {
        return Ok(Space::Special {
            inputs: count,
            all: false,
        });
    }
    tint_space(
        document,
        resources,
        base,
        function,
        document
            .resolve(names)?
            .as_array()
            .unwrap_or_default()
            .iter()
            .map(|value| {
                document
                    .resolve(value)?
                    .as_name()
                    .cloned()
                    .ok_or(Error::syntax("DeviceN colourant must be a name"))
            })
            .collect::<Result<Vec<_>>>()?,
        (depth, defaults),
    )
}

fn device_n_components(document: &ColorContext<'_>, names: &Value) -> Result<usize> {
    let names = document
        .resolve(names)?
        .as_array()
        .filter(|n| !n.is_empty())
        .ok_or(Error::syntax("DeviceN requires colourant names"))?;
    // 32 is this renderer's implementation budget, not a format maximum;
    // Annex C / Table C.1 discusses practical processor limits.
    if names.len() > 32 {
        return Err(Error::limit(
            "DeviceN component count exceeds implementation limit (Annex C)",
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    for name in names {
        let name = document
            .resolve(name)?
            .as_name()
            .ok_or(Error::syntax("DeviceN colourant must be a name"))?;
        if name == b"All" {
            return Err(Error::syntax("All is forbidden in DeviceN"));
        }
        if name != b"None" && !seen.insert(name.as_bytes()) {
            return Err(Error::syntax("duplicate DeviceN colourant"));
        }
    }
    Ok(names.len())
}

fn tint_space(
    document: &ColorContext<'_>,
    resources: &Dictionary,
    base: &Value,
    function: &Value,
    names: Vec<pagura_core::Name>,
    (depth, defaults): (usize, bool),
) -> Result<Space> {
    let base = resolve(document, resources, base, depth + 1, defaults)?;
    if matches!(
        base,
        Space::Indexed { .. } | Space::Tint { .. } | Space::Special { .. } | Space::Pattern(_)
    ) {
        return Err(Error::syntax("invalid tint alternate space"));
    }
    let function =
        crate::functions::Function::parse(document, function, names.len(), base.components())?;
    Ok(Space::Tint {
        names: names.into(),
        base: Arc::new(base),
        function: Arc::new(function),
    })
}

fn icc_space(
    document: &ColorContext<'_>,
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
    let component_range = icc_range(document, dictionary, default.components())?;
    let profile = document.profile(id, default.components())?;
    // Validate Alternate independently of whether its rendering model is used.
    // A supported ICC profile must not require support for an unused alternate.
    if let Some(alternate) = dictionary.get(b"Alternate") {
        if alternate_components(document, resources, alternate, depth + 1)? != default.components()
        {
            return Err(Error::syntax("ICC alternate has wrong component count"));
        }
        if profile.is_some()
            && let Err(error) = resolve(document, resources, alternate, depth + 1, false)
            && !error.is_unsupported()
        {
            return Err(error);
        }
    }
    if profile.as_ref().is_some_and(|p| p.lab_input()) {
        for (i, pair) in component_range.iter().take(3).enumerate() {
            let expected = if i == 0 {
                [0.0, 100.0]
            } else {
                [-128.0, 127.0]
            };
            if pair[0] < expected[0] || pair[1] > expected[1] {
                return Err(Error::syntax("ICC Range exceeds Lab profile range"));
            }
        }
    } else if profile.is_some()
        && component_range
            .iter()
            .take(default.components())
            .any(|p| p[0] < 0.0 || p[1] > 1.0)
    {
        return Err(Error::syntax("ICC Range exceeds profile range"));
    }
    let base = if profile.is_some() {
        default
    } else {
        dictionary
            .get(b"Alternate")
            .map(|v| resolve(document, resources, v, depth + 1, false))
            .transpose()?
            .unwrap_or(default)
    };
    Ok(Space::Icc {
        base: Arc::new(base),
        profile,
        range: component_range,
    })
}

fn icc_range(
    document: &pagura_document::Document,
    dictionary: &Dictionary,
    count: usize,
) -> Result<[[f64; 2]; 4]> {
    let mut component_range = [[0.0, 1.0]; 4];
    if let Some(range) = dictionary.get(b"Range") {
        let range = document
            .resolve(range)?
            .as_array()
            .ok_or(Error::syntax("ICC Range must be an array"))?;
        let expected = count * 2;
        if range.len() != expected {
            return Err(Error::syntax("wrong ICC Range length"));
        }
        for (i, pair) in range.as_chunks::<2>().0.iter().enumerate() {
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
            component_range[i] = [low, high];
        }
    }
    Ok(component_range)
}

fn alternate_components(
    document: &ColorContext<'_>,
    resources: &Dictionary,
    value: &Value,
    depth: usize,
) -> Result<usize> {
    if depth > document.limits().depth {
        return Err(Error::limit("ICC alternate depth exceeded"));
    }
    let value = document.resolve(value)?;
    let (family, args) = if let Some(name) = value.as_name() {
        (name.as_bytes(), &[][..])
    } else {
        let array = value
            .as_array()
            .ok_or(Error::syntax("invalid ICC alternate"))?;
        let family = array
            .first()
            .and_then(Value::as_name)
            .ok_or(Error::syntax("invalid ICC alternate family"))?;
        (family.as_bytes(), &array[1..])
    };
    Ok(match family {
        b"DeviceGray" | b"CalGray" | b"Indexed" | b"Separation" => 1,
        b"DeviceRGB" | b"CalRGB" | b"Lab" => 3,
        b"DeviceCMYK" => 4,
        b"DeviceN" => document
            .resolve(
                args.first()
                    .ok_or(Error::syntax("missing ICC alternate DeviceN names"))?,
            )?
            .as_array()
            .ok_or(Error::syntax("invalid ICC alternate DeviceN names"))?
            .len(),
        b"ICCBased" => {
            let id = args
                .first()
                .and_then(Value::as_reference)
                .ok_or(Error::syntax("invalid ICC alternate profile reference"))?;
            let object = document.object(id)?;
            let dictionary = object
                .dictionary()
                .filter(|_| object.is_stream())
                .ok_or(Error::syntax("invalid ICC alternate profile"))?;
            let n = document
                .resolve(
                    dictionary
                        .get(b"N")
                        .ok_or(Error::syntax("missing ICC alternate N"))?,
                )?
                .as_integer()
                .filter(|v| matches!(v, 1 | 3 | 4))
                .ok_or(Error::syntax("invalid ICC alternate N"))?;
            usize::try_from(n).map_err(|_| Error::syntax("invalid ICC alternate N"))?
        }
        b"Pattern" => return Err(Error::syntax("Pattern is not an ICC alternate")),
        name if args.is_empty() => {
            let spaces = resources
                .get(b"ColorSpace")
                .map(|v| document.resolve_dictionary(v))
                .transpose()?
                .ok_or(Error::syntax("undefined ICC alternate"))?;
            let value = spaces
                .get(name)
                .ok_or(Error::syntax("undefined ICC alternate"))?;
            return alternate_components(document, resources, value, depth + 1);
        }
        _ => return Err(Error::syntax("invalid ICC alternate family")),
    })
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

fn indexed_space(
    document: &ColorContext<'_>,
    resources: &Dictionary,
    [base, high, lookup]: [&Value; 3],
    depth: usize,
    defaults: bool,
) -> Result<Space> {
    let base = resolve(document, resources, base, depth + 1, defaults)?;
    if matches!(base, Space::Indexed { .. } | Space::Pattern(_)) {
        return Err(Error::syntax("Indexed base cannot be Indexed"));
    }
    let high = document
        .resolve(high)?
        .as_integer()
        .and_then(|n| u8::try_from(n).ok())
        .ok_or(Error::syntax("invalid Indexed high value"))?;
    let lookup_id = lookup.as_reference();
    let mut lookup = if let Some(id) = lookup_id {
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
        return Err(Error::syntax("Indexed lookup has wrong length"));
    }
    if lookup.len() > required {
        let mut diagnostic = pagura_core::Diagnostic::new(
            crate::interpreter::tags::INDEXED_EXCESS,
            "Indexed lookup has wrong length",
        );
        if let Some(id) = lookup_id {
            diagnostic = diagnostic.in_object(id);
        }
        crate::interpreter::repair(document, diagnostic)?;
        lookup.truncate(required);
    }
    Ok(Space::Indexed {
        base: Arc::new(base),
        high,
        lookup: lookup.into(),
    })
}
