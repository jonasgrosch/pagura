//! Blend functions in additive coordinates (11.3.5, Tables 134–135).

use crate::transparency::Model;
use pagura_core::{Error, Result, Value};
use pagura_document::Document;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum BlendMode {
    #[default]
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    ColorDodge,
    ColorBurn,
    HardLight,
    SoftLight,
    Difference,
    Exclusion,
    Hue,
    Saturation,
    Color,
    Luminosity,
}

impl BlendMode {
    pub(crate) fn parse(document: &Document, value: &Value) -> Result<Self> {
        let value = document.resolve(value)?;
        if let Some(names) = value.as_array() {
            let mut selected = None;
            for value in names {
                let name = document
                    .resolve(value)?
                    .as_name()
                    .ok_or(Error::syntax("blend array requires names"))?;
                if selected.is_none() {
                    selected = Self::named(name.as_bytes());
                }
            }
            return Ok(selected.unwrap_or_default());
        }
        let name = value
            .as_name()
            .ok_or(Error::syntax("blend mode must be a name or array"))?;
        Self::named(name.as_bytes()).ok_or(Error::unsupported(
            crate::features::TRANSPARENCY_BM,
            "unknown blend mode",
        ))
    }
    fn named(name: &[u8]) -> Option<Self> {
        Some(match name {
            b"Normal" | b"Compatible" => Self::Normal,
            b"Multiply" => Self::Multiply,
            b"Screen" => Self::Screen,
            b"Overlay" => Self::Overlay,
            b"Darken" => Self::Darken,
            b"Lighten" => Self::Lighten,
            b"ColorDodge" => Self::ColorDodge,
            b"ColorBurn" => Self::ColorBurn,
            b"HardLight" => Self::HardLight,
            b"SoftLight" => Self::SoftLight,
            b"Difference" => Self::Difference,
            b"Exclusion" => Self::Exclusion,
            b"Hue" => Self::Hue,
            b"Saturation" => Self::Saturation,
            b"Color" => Self::Color,
            b"Luminosity" => Self::Luminosity,
            _ => return None,
        })
    }
    /// The blended colour of backdrop `back` and `source` in the group's
    /// colour model (11.3.5).
    pub(crate) fn apply(self, back: [f64; 4], source: [f64; 4], model: Model) -> [f64; 4] {
        if matches!(
            self,
            Self::Hue | Self::Saturation | Self::Color | Self::Luminosity
        ) {
            let gray = model == Model::Gray;
            let rgb = |c: [f64; 4]| {
                if gray { [c[0]; 3] } else { [c[0], c[1], c[2]] }
            };
            let (b, s) = (rgb(back), rgb(source));
            let result = match self {
                Self::Hue => set_lum(set_sat(s, saturation(b)), lum(b)),
                Self::Saturation => set_lum(set_sat(b, saturation(s)), lum(b)),
                Self::Color => set_lum(s, lum(b)),
                _ => set_lum(b, lum(s)),
            };
            return if gray {
                [lum(result), 0.0, 0.0, 0.0]
            } else {
                [
                    result[0],
                    result[1],
                    result[2],
                    if self == Self::Luminosity {
                        source[3]
                    } else {
                        back[3]
                    },
                ]
            };
        }
        [0, 1, 2, 3].map(|i| self.component(back[i], source[i]))
    }
    fn component(self, b: f64, s: f64) -> f64 {
        match self {
            Self::Multiply => b * s,
            Self::Screen => b + s - b * s,
            Self::Overlay => hard(s, b),
            Self::Darken => b.min(s),
            Self::Lighten => b.max(s),
            // PDF 2.0 defines these boundary cases before testing the divisor.
            Self::ColorDodge => {
                if b == 0.0 {
                    0.0
                } else if b >= 1.0 - s {
                    1.0
                } else {
                    b / (1.0 - s)
                }
            }
            Self::ColorBurn => {
                if b == 1.0 {
                    1.0
                } else if 1.0 - b >= s {
                    0.0
                } else {
                    1.0 - (1.0 - b) / s
                }
            }
            Self::HardLight => hard(b, s),
            Self::SoftLight => {
                if s <= 0.5 {
                    b - (1.0 - 2.0 * s) * b * (1.0 - b)
                } else {
                    let d = if b <= 0.25 {
                        ((16.0 * b - 12.0) * b + 4.0) * b
                    } else {
                        b.sqrt()
                    };
                    b + (2.0 * s - 1.0) * (d - b)
                }
            }
            Self::Difference => (b - s).abs(),
            Self::Exclusion => b + s - 2.0 * b * s,
            _ => s,
        }
    }
}
fn hard(b: f64, s: f64) -> f64 {
    if s <= 0.5 {
        2.0 * b * s
    } else {
        1.0 - 2.0 * (1.0 - b) * (1.0 - s)
    }
}
pub(crate) fn lum(c: [f64; 3]) -> f64 {
    0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2]
}
fn saturation(c: [f64; 3]) -> f64 {
    c.into_iter().fold(f64::NEG_INFINITY, f64::max) - c.into_iter().fold(f64::INFINITY, f64::min)
}
fn set_lum(c: [f64; 3], target: f64) -> [f64; 3] {
    let d = target - lum(c);
    let mut c = c.map(|v| v + d);
    let low = c.into_iter().fold(f64::INFINITY, f64::min);
    let high = c.into_iter().fold(f64::NEG_INFINITY, f64::max);
    if low < 0.0 {
        c = c.map(|v| target + (v - target) * target / (target - low));
    }
    if high > 1.0 {
        c = c.map(|v| target + (v - target) * (1.0 - target) / (high - target));
    }
    c.map(|v| v.clamp(0.0, 1.0))
}
fn set_sat(c: [f64; 3], amount: f64) -> [f64; 3] {
    let mut order = [0, 1, 2];
    order.sort_unstable_by(|&a, &b| c[a].total_cmp(&c[b]));
    let [low, middle, high] = order;
    let mut result = [0.0; 3];
    if c[high] > c[low] {
        result[middle] = (c[middle] - c[low]) * amount / (c[high] - c[low]);
        result[high] = amount;
    }
    result
}
