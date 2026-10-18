//! CIE-based colour space dictionaries (8.6.5.2–8.6.5.4). The colorimetry
//! is `pagura_color::cie`; this module reads the PDF parameters.

use crate::color::Components;
use crate::numbers;
use pagura_color::cie::Calibrated;
use pagura_core::{Dictionary, Error, Result};
use pagura_document::Document;

/// A validated CIE-based space.
pub(crate) type Cie = Calibrated;

/// Parameters outside this magnitude have no meaningful colorimetry.
const RANGE: f64 = 1e6;

/// Reads a `CalGray`, `CalRGB` or `Lab` parameter dictionary.
///
/// # Errors
/// As [`Calibrated`]'s constructors, plus `Syntax` for malformed entries.
pub(crate) fn parse(document: &Document, family: &[u8], d: &Dictionary) -> Result<Cie> {
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
        b"Lab" => {
            let range =
                parameter::<4>(document, d, b"Range", Some([-100.0, 100.0, -100.0, 100.0]))?;
            Calibrated::lab(white, black, range)
        }
        _ => Err(Error::syntax("invalid calibrated colour family")),
    }
}

/// Maps Indexed lookup bytes to the space's source range (8.6.6.3).
pub(crate) fn decode_lookup(space: &Cie, bytes: &[u8]) -> Components {
    let n = |i: usize| f64::from(bytes.get(i).copied().unwrap_or(0)) / 255.0;
    match (space.components(), space.lab_range()) {
        (_, Some(r)) => [
            100.0 * n(0),
            r[0] + n(1) * (r[1] - r[0]),
            r[2] + n(2) * (r[3] - r[2]),
        ]
        .into_iter()
        .collect(),
        (1, None) => [n(0)].into_iter().collect(),
        _ => [n(0), n(1), n(2)].into_iter().collect(),
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
    numbers::array(document, value, RANGE)
}
