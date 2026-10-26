//! The previous colour conversion (`pagura-render` `color.rs`,
//! `transparency.rs` and `compositor/colorants.rs` at the start of the
//! one-pass refactor), ported as a test-only differential oracle:
//! `Space::convert_with_intent`, `Model::{source, native, encode_rgb}`,
//! Indexed decoding and the separation pass logic of `Context::source` and
//! `sampled_source`, with their arithmetic and error returns kept.
#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the ported casts are bounded as they were in the previous code"
)]

use crate::cie::Calibrated;
use crate::function::tests::oracle::Oracle;
use crate::icc::{Intent, Profile};
use pagura_core::{Error, Name, Result};
use std::sync::Arc;

/// A colour space as the previous resolution produced it.
#[derive(Debug, Clone)]
pub(crate) enum Space {
    Special {
        inputs: usize,
        all: bool,
    },
    Gray,
    Rgb,
    Cmyk,
    Cie(Arc<Calibrated>),
    Icc {
        base: Arc<Self>,
        profile: Option<Arc<Profile>>,
        range: [[f64; 2]; 4],
    },
    Tint {
        names: Arc<[Name]>,
        base: Arc<Self>,
        function: Arc<Oracle>,
    },
    Indexed {
        base: Arc<Self>,
        high: u8,
        lookup: Arc<[u8]>,
    },
}

/// The previous blending model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Model {
    Gray,
    Rgb,
    Cmyk,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Conversion {
    pub(crate) intent: Intent,
    pub(crate) black: Option<Arc<Oracle>>,
    pub(crate) ucr: Option<Arc<Oracle>>,
}

fn lum(c: [f64; 3]) -> f64 {
    0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2]
}

impl Space {
    fn clip_icc(c: &[f64], range: &[[f64; 2]; 4]) -> [f64; 4] {
        [0, 1, 2, 3].map(|i| {
            c.get(i)
                .copied()
                .unwrap_or(0.0)
                .clamp(range[i][0], range[i][1])
        })
    }

    fn decode_lookup(&self, bytes: &[u8]) -> Vec<f64> {
        match self {
            Self::Cie(cie) => {
                let n = |i: usize| f64::from(bytes.get(i).copied().unwrap_or(0)) / 255.0;
                match (cie.components(), cie.lab_ranges()) {
                    (_, Some(r)) => vec![
                        r[0][0] + n(0) * (r[0][1] - r[0][0]),
                        r[1][0] + n(1) * (r[1][1] - r[1][0]),
                        r[2][0] + n(2) * (r[2][1] - r[2][0]),
                    ],
                    (1, None) => vec![n(0)],
                    _ => vec![n(0), n(1), n(2)],
                }
            }
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

    pub(crate) fn components(&self) -> usize {
        match self {
            Self::Gray | Self::Indexed { .. } => 1,
            Self::Rgb => 3,
            Self::Cmyk => 4,
            Self::Cie(cie) => cie.components(),
            Self::Icc { base, .. } => base.components(),
            Self::Tint { names, .. } => names.len(),
            Self::Special { inputs, .. } => *inputs,
        }
    }

    fn convert_with_intent(&self, c: &[f64], intent: Intent) -> Result<[f64; 3]> {
        let n = |i: usize| c.get(i).copied().unwrap_or(0.0).clamp(0.0, 1.0);
        match self {
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
                base.convert_with_intent(&function.evaluate_with_work(&tint, &mut 0)?, intent)
            }
            Self::Indexed { base, high, lookup } => {
                let components =
                    indexed_components(base, *high, lookup, c.first().copied().unwrap_or(0.0))?;
                base.convert_with_intent(&components, intent)
            }
        }
    }
}

fn indexed_components(base: &Space, high: u8, lookup: &[u8], value: f64) -> Result<Vec<f64>> {
    let value = value.round().clamp(0.0, f64::from(high));
    let index = value as usize;
    let count = base.components();
    let bytes = lookup
        .get(index * count..(index + 1) * count)
        .ok_or(Error::syntax("truncated Indexed colour lookup"))?;
    Ok(base.decode_lookup(bytes))
}

fn indexed<'a>(space: &'a Space, components: &[f64]) -> Result<Option<(&'a Space, Vec<f64>)>> {
    let Space::Indexed { base, high, lookup } = space else {
        return Ok(None);
    };
    Ok(Some((
        base,
        indexed_components(
            base,
            *high,
            lookup,
            components.first().copied().unwrap_or(0.0),
        )?,
    )))
}

/// Native component conversion and zero selection.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Native {
    pub(crate) color: [f64; 4],
    pub(crate) zero: [bool; 4],
}

impl Model {
    pub(crate) fn components(self) -> usize {
        match self {
            Self::Gray => 1,
            Self::Rgb => 3,
            Self::Cmyk => 4,
        }
    }

    fn encode_rgb(
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

    pub(crate) fn source(
        self,
        space: &Space,
        components: &[f64],
        conversion: &Conversion,
        work: &mut usize,
    ) -> Result<[f64; 4]> {
        if let Some(native) = self.native(space, components) {
            return Ok(native.color);
        }
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
        if let Some((base, decoded)) = indexed(space, components)? {
            return self.source(base, &decoded, conversion, work);
        }
        self.encode_rgb(
            space.convert_with_intent(components, conversion.intent)?,
            conversion,
            work,
        )
    }

    pub(crate) fn native(self, space: &Space, components: &[f64]) -> Option<Native> {
        if let Space::Icc {
            base,
            profile: None,
            range,
        } = space
        {
            let clipped = Space::clip_icc(components, range);
            return self.native(base, &clipped[..base.components()]);
        }
        if !matches!(
            (self, space),
            (Self::Gray, Space::Gray) | (Self::Rgb, Space::Rgb) | (Self::Cmyk, Space::Cmyk)
        ) {
            return None;
        }
        let mut native = Native {
            color: [0.0; 4],
            zero: [false; 4],
        };
        for ((color, zero), &v) in native
            .color
            .iter_mut()
            .zip(&mut native.zero)
            .zip(components)
        {
            let v = v.clamp(0.0, 1.0);
            *zero = v == 0.0;
            *color = if self == Self::Cmyk { 1.0 - v } else { v };
        }
        Some(native)
    }
}

fn scalar(function: Option<&Oracle>, value: f64, work: &mut usize) -> Result<f64> {
    let mut result = [value];
    if let Some(function) = function {
        function.evaluate_into(&[value], &mut result, work)?;
    }
    Ok(result[0])
}

/// The page-level separation state of the previous compositor.
pub(crate) struct Separations<'a> {
    /// Whether separations were enabled for the page.
    pub(crate) enabled: bool,
    pub(crate) spots: &'a [Name],
}

/// The mark state the previous colour selection read.
#[derive(Debug, Clone, Copy)]
pub(crate) struct State {
    pub(crate) overprint: bool,
    pub(crate) nonzero: bool,
    pub(crate) direct: bool,
}

/// One pass's colour and preserved channels.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Pass {
    pub(crate) color: [f64; 4],
    pub(crate) preserve: [bool; 4],
}

fn process(name: &Name) -> Option<usize> {
    [b"Cyan".as_slice(), b"Magenta", b"Yellow", b"Black"]
        .iter()
        .position(|&n| name == n)
}

/// `Context::source` for the process pass (`batch` `None`) or a spot batch,
/// with overprint simulated.
pub(crate) fn context_source(
    space: &Space,
    components: &[f64],
    model: Model,
    (separations, state, batch): (&Separations<'_>, State, Option<usize>),
    conversion: &Conversion,
    work: &mut usize,
) -> Result<Pass> {
    if let Some((base, decoded)) = indexed(space, components)? {
        return context_source(
            base,
            &decoded,
            model,
            (separations, state, batch),
            conversion,
            work,
        );
    }
    let spot = batch.is_some();
    let tint_index = |name: &Name| -> Option<usize> {
        if let Some(batch) = batch {
            separations
                .spots
                .iter()
                .position(|s| *s == *name)
                .and_then(|i| i.checked_sub(batch * 4))
                .filter(|&i| i < 4)
        } else {
            process(name)
        }
    };
    let native_tint = if separations.enabled
        && let Space::Tint { names, .. } = space
        && (model == Model::Cmyk || names.iter().all(|n| process(n).is_none()))
    {
        Some(names)
    } else {
        None
    };
    // sampled_source
    let mut preserve = [false; 4];
    if state.overprint {
        let mut specified = [!spot; 4];
        if let Some(names) = native_tint {
            specified = [false; 4];
            for name in names.iter() {
                if let Some(i) = tint_index(name) {
                    specified[i] = true;
                }
            }
        } else if matches!(space, Space::Special { all: true, .. }) {
            specified = [true; 4];
        }
        preserve = specified.map(|v| !v);
    }
    let mut color = if spot || native_tint.is_some() {
        [1.0; 4]
    } else {
        model.source(space, components, conversion, work)?
    };
    if let Space::Special { all: true, .. } = space
        && spot
    {
        color = [1.0 - components.first().copied().unwrap_or(1.0).clamp(0.0, 1.0); 4];
    }
    if let Some(names) = native_tint {
        for (name, &tint) in names.iter().zip(components) {
            if let Some(i) = tint_index(name) {
                color[i] = 1.0 - tint.clamp(0.0, 1.0);
            }
        }
    }
    // The previous code tested the Simulate policy here; the new target
    // simulates overprint exactly when separations are enabled.
    if separations.enabled
        && model == Model::Cmyk
        && !spot
        && state.direct
        && state.overprint
        && state.nonzero
        && let Some(native) = model.native(space, components)
    {
        preserve = native.zero;
    }
    Ok(Pass { color, preserve })
}
