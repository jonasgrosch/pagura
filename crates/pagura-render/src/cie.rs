//! CIE colour conversion (8.6.5.2–8.6.5.4).
//!
//! Relative-colourimetric output uses Bradford chromatic adaptation from the
//! declared white to D65, then the IEC 61966-2-1 sRGB matrix/transfer curve.
//! This is an explicit output-device choice, not ICC profile processing.
//! Nonzero BlackPoint needs black-point compensation and remains unsupported.

use crate::features;
use pagura_color::cie::{adaptation, lab_inverse, map, xyz_to_srgb};
use pagura_core::{Dictionary, Error, Result};
use pagura_document::Document;

#[derive(Debug, Clone)]
enum Definition {
    Gray(f64),
    Rgb {
        gamma: [f64; 3],
        matrix: [[f64; 3]; 3],
    },
    Lab([f64; 4]),
}

#[derive(Debug, Clone)]
pub(crate) struct Cie {
    definition: Definition,
    white: [f64; 3],
    adaptation: [[f64; 3]; 3],
}

impl Cie {
    pub(crate) fn parse(
        document: &Document,
        family: &[u8],
        dictionary: &Dictionary,
    ) -> Result<Self> {
        let white = numbers::<3>(document, dictionary, b"WhitePoint", None)?;
        if white[0] <= 0.0 || white[1] != 1.0 || white[2] <= 0.0 {
            return Err(Error::syntax("invalid CIE white point"));
        }
        let black = numbers::<3>(document, dictionary, b"BlackPoint", Some([0.0; 3]))?;
        if black.iter().any(|&v| v < 0.0) {
            return Err(Error::syntax("negative CIE black point"));
        }
        if black != [0.0; 3] {
            return Err(Error::unsupported(
                features::COLORSPACE_BLACKPOINT,
                "nonzero CIE black point is not implemented",
            ));
        }
        let definition = match family {
            b"CalGray" => {
                let gamma = dictionary
                    .get(b"Gamma")
                    .map(|v| {
                        document
                            .resolve(v)?
                            .as_number()
                            .ok_or(Error::syntax("CalGray Gamma must be numeric"))
                    })
                    .transpose()?
                    .unwrap_or(1.0);
                if gamma <= 0.0 {
                    return Err(Error::syntax("CalGray Gamma must be positive"));
                }
                Definition::Gray(gamma)
            }
            b"Lab" => {
                let range = numbers::<4>(
                    document,
                    dictionary,
                    b"Range",
                    Some([-100.0, 100.0, -100.0, 100.0]),
                )?;
                if range[0] > range[1] || range[2] > range[3] {
                    return Err(Error::syntax("invalid Lab range"));
                }
                Definition::Lab(range)
            }
            b"CalRGB" => calibrated_rgb(document, dictionary)?,
            _ => return Err(Error::syntax("invalid calibrated colour family")),
        };
        let adaptation = adaptation(white)?;
        Ok(Self {
            definition,
            white,
            adaptation,
        })
    }
    pub(crate) fn is_lab(&self) -> bool {
        matches!(self.definition, Definition::Lab(_))
    }
    pub(crate) fn components(&self) -> usize {
        match self.definition {
            Definition::Gray(_) => 1,
            Definition::Lab(_) | Definition::Rgb { .. } => 3,
        }
    }
    pub(crate) fn convert(&self, c: &[f64]) -> [f64; 3] {
        let n = |i: usize| c.get(i).copied().unwrap_or(0.0);
        let xyz = match self.definition {
            Definition::Gray(gamma) => self.white.map(|w| w * n(0).clamp(0.0, 1.0).powf(gamma)),
            Definition::Rgb { gamma, matrix } => map(
                matrix,
                [0, 1, 2].map(|i| n(i).clamp(0.0, 1.0).powf(gamma[i])),
            ),
            Definition::Lab(range) => {
                let m = (n(0).clamp(0.0, 100.0) + 16.0) / 116.0;
                let a = n(1).clamp(range[0], range[1]);
                let b = n(2).clamp(range[2], range[3]);
                [
                    self.white[0] * lab_inverse(m + a / 500.0),
                    lab_inverse(m),
                    self.white[2] * lab_inverse(m - b / 200.0),
                ]
            }
        };
        let xyz = map(self.adaptation, xyz);
        xyz_to_srgb(xyz)
    }
    pub(crate) fn decode_lookup(&self, bytes: &[u8]) -> Vec<f64> {
        let n = |i: usize| f64::from(bytes.get(i).copied().unwrap_or(0)) / 255.0;
        match self.definition {
            Definition::Gray(_) => vec![n(0)],
            Definition::Rgb { .. } => vec![n(0), n(1), n(2)],
            Definition::Lab(r) => vec![
                100.0 * n(0),
                r[0] + n(1) * (r[1] - r[0]),
                r[2] + n(2) * (r[3] - r[2]),
            ],
        }
    }
}

/// CalRGB's column-major gamma/matrix transformation to XYZ (8.6.5.3).
fn calibrated_rgb(document: &Document, d: &Dictionary) -> Result<Definition> {
    let gamma = numbers::<3>(document, d, b"Gamma", Some([1.0; 3]))?;
    if gamma.iter().any(|&v| v <= 0.0) {
        return Err(Error::syntax("CalRGB Gamma must be positive"));
    }
    let m = numbers::<9>(
        document,
        d,
        b"Matrix",
        Some([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]),
    )?;
    Ok(Definition::Rgb {
        gamma,
        matrix: [[m[0], m[3], m[6]], [m[1], m[4], m[7]], [m[2], m[5], m[8]]],
    })
}

fn numbers<const N: usize>(
    document: &Document,
    d: &Dictionary,
    key: &[u8],
    default: Option<[f64; N]>,
) -> Result<[f64; N]> {
    let Some(value) = d.get(key) else {
        return default.ok_or(Error::syntax("missing CIE parameter"));
    };
    let values = document
        .resolve(value)?
        .as_array()
        .filter(|v| v.len() == N)
        .ok_or(Error::syntax("invalid CIE parameter array"))?;
    let mut out = [0.0; N];
    for (out, v) in out.iter_mut().zip(values) {
        *out = document
            .resolve(v)?
            .as_number()
            .ok_or(Error::syntax("CIE parameter must be numeric"))?;
        if out.abs() > 1e6 {
            return Err(Error::limit("CIE parameter exceeds numeric working range"));
        }
    }
    Ok(out)
}
