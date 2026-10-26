//! CIE colorimetry shared by calibrated PDF spaces and ICC profiles.
//!
//! Output is IEC 61966-2-1 sRGB: Bradford chromatic adaptation from a
//! source white to D65, the sRGB matrix, gamut clipping and the sRGB
//! transfer curve. This is an output-device choice, not ICC processing.

mod bounds;

use crate::affine::{Affine, Undecided};
use crate::features;
use pagura_core::{Error, Result};

pub use bounds::encode_bounds;
pub(crate) use bounds::{MARGIN, power_bounds, power_cubic};

/// A 3×3 matrix applied to column vectors, row by row.
pub type Matrix = [[f64; 3]; 3];

/// The ICC profile connection space illuminant (ICC.1:2010 7.2.16).
pub const D50: [f64; 3] = [0.9642, 1.0, 0.8249];

/// The sRGB reference white (IEC 61966-2-1).
pub const D65: [f64; 3] = [0.95047, 1.0, 1.08883];

const BRADFORD: Matrix = [
    [0.8951, 0.2664, -0.1614],
    [-0.7502, 1.7135, 0.0367],
    [0.0389, -0.0685, 1.0296],
];
const BRADFORD_INVERSE: Matrix = [
    [0.986_992_9, -0.147_054_3, 0.159_962_7],
    [0.432_305_3, 0.518_360_3, 0.049_291_2],
    [-0.008_528_7, 0.040_042_8, 0.968_486_7],
];
const XYZ_TO_LINEAR_SRGB: Matrix = [
    [3.240_454_2, -1.537_138_5, -0.498_531_4],
    [-0.969_266, 1.876_010_8, 0.041_556],
    [0.055_643_4, -0.204_025_9, 1.057_225_2],
];

/// Bradford adaptation from the ICC PCS illuminant to D65, used by every
/// ICC profile.
pub const D50_TO_D65: Matrix = bradford(D50);

/// Multiplies a matrix by a column vector.
pub const fn map(matrix: Matrix, value: [f64; 3]) -> [f64; 3] {
    let mut result = [0.0; 3];
    let mut row = 0;
    while row < 3 {
        result[row] =
            matrix[row][0] * value[0] + matrix[row][1] * value[1] + matrix[row][2] * value[2];
        row += 1;
    }
    result
}

/// Bradford chromatic adaptation from `white` to D65.
///
/// # Errors
/// Returns `Limit` for a white point whose cone response is singular.
pub fn adaptation(white: [f64; 3]) -> Result<Matrix> {
    if map(BRADFORD, white).iter().any(|v| v.abs() < 1e-12) {
        return Err(Error::limit("CIE white adaptation is singular"));
    }
    Ok(bradford(white))
}

/// The Bradford matrix for a source white with a nonsingular cone response.
const fn bradford(white: [f64; 3]) -> Matrix {
    let source = map(BRADFORD, white);
    let target = map(BRADFORD, D65);
    let ratio = [
        target[0] / source[0],
        target[1] / source[1],
        target[2] / source[2],
    ];
    let mut result = [[0.0; 3]; 3];
    let mut row = 0;
    while row < 3 {
        let mut column = 0;
        while column < 3 {
            let mut k = 0;
            while k < 3 {
                result[row][column] += BRADFORD_INVERSE[row][k] * ratio[k] * BRADFORD[k][column];
                k += 1;
            }
            column += 1;
        }
        row += 1;
    }
    result
}

/// CIELAB to XYZ relative to `white` (CIE 15:2004 8.2.1.1).
pub fn lab_to_xyz([l, a, b]: [f64; 3], white: [f64; 3]) -> [f64; 3] {
    // The inverse companding function; both branches meet at 6/29.
    let inverse = |v: f64| {
        if v > 6.0 / 29.0 {
            v * v * v
        } else {
            (v - 4.0 / 29.0) * (108.0 / 841.0)
        }
    };
    let y = (l + 16.0) / 116.0;
    [
        white[0] * inverse(y + a / 500.0),
        white[1] * inverse(y),
        white[2] * inverse(y - b / 200.0),
    ]
}

/// Converts D65-relative XYZ to gamut-clipped, encoded sRGB.
pub fn xyz_to_srgb(xyz: [f64; 3]) -> [f64; 3] {
    linear_srgb(xyz).map(encode)
}

/// Converts D65-relative XYZ to gamut-clipped linear sRGB, which [`encode`]
/// takes to encoded sRGB.
pub fn linear_srgb(xyz: [f64; 3]) -> [f64; 3] {
    map(XYZ_TO_LINEAR_SRGB, xyz).map(|v| v.clamp(0.0, 1.0))
}

/// Encloses [`map`] (see [`crate::affine`]).
pub(crate) fn enclose_map(matrix: Matrix, value: [Affine; 3]) -> [Affine; 3] {
    matrix.map(|row| {
        value[0]
            .scale(row[0])
            .add(value[1].scale(row[1]))
            .add(value[2].scale(row[2]))
    })
}

/// Encloses [`linear_srgb`].
pub(crate) fn enclose_linear_srgb(xyz: [Affine; 3]) -> Result<[Affine; 3], Undecided> {
    let [r, g, b] = enclose_map(XYZ_TO_LINEAR_SRGB, xyz);
    Ok([r.clamp(0.0, 1.0)?, g.clamp(0.0, 1.0)?, b.clamp(0.0, 1.0)?])
}

/// The branch [`encode`] takes for every value of `v`: the enclosure of
/// the encoded value where every value is scaled, `None` where every value
/// is raised to the power.
pub(crate) fn enclose_encode(v: Affine) -> Result<Option<Affine>, Undecided> {
    if v.high() <= LINEAR_LIMIT {
        Ok(Some(v.scale(12.92)))
    } else if v.low() > LINEAR_LIMIT {
        Ok(None)
    } else {
        Err(Undecided::Split {
            form: v,
            at: LINEAR_LIMIT,
        })
    }
}

/// The sRGB transfer function (IEC 61966-2-1) of a linear value in `0..=1`.
pub fn encode(v: f64) -> f64 {
    if v <= LINEAR_LIMIT {
        12.92 * v
    } else {
        affine(v.powf(1.0 / 2.4))
    }
}

/// The largest linear value [`encode`] scales instead of raising.
const LINEAR_LIMIT: f64 = 0.003_130_8;

/// The encoding of a power `p = v^(1/2.4)`: increasing in `p`, also as
/// rounded arithmetic.
fn affine(p: f64) -> f64 {
    1.055 * p - 0.055
}

/// A CIE-based PDF colour space (8.6.5.2–8.6.5.4) after parameter validation.
#[derive(Debug, Clone)]
pub struct Calibrated {
    model: Model,
    white: [f64; 3],
    adaptation: Matrix,
}

#[derive(Debug, Clone, Copy)]
enum Model {
    Gray { gamma: f64 },
    Rgb { gamma: [f64; 3], matrix: Matrix },
    Lab { range: [f64; 4] },
}

impl Calibrated {
    /// A CalGray space (8.6.5.2).
    ///
    /// # Errors
    /// As [`Self::rgb`], for a nonpositive `gamma`.
    pub fn gray(white: [f64; 3], black: [f64; 3], gamma: f64) -> Result<Self> {
        check_points(white, black)?;
        if gamma <= 0.0 {
            return Err(Error::syntax("CalGray Gamma must be positive"));
        }
        Self::new(Model::Gray { gamma }, white)
    }

    /// A CalRGB space (8.6.5.3). `matrix` maps linear components to XYZ,
    /// row by row (the transpose of PDF's column-major `Matrix` array).
    ///
    /// # Errors
    /// `Syntax` for an invalid white point, a negative black point or a
    /// nonpositive gamma; `Unsupported` for a nonzero black point, which
    /// needs black-point compensation; `Limit` for a singular white point.
    pub fn rgb(white: [f64; 3], black: [f64; 3], gamma: [f64; 3], matrix: Matrix) -> Result<Self> {
        check_points(white, black)?;
        if gamma.iter().any(|&v| v <= 0.0) {
            return Err(Error::syntax("CalRGB Gamma must be positive"));
        }
        Self::new(Model::Rgb { gamma, matrix }, white)
    }

    /// A Lab space (8.6.5.4); `range` is `[a_min, a_max, b_min, b_max]`.
    ///
    /// # Errors
    /// As [`Self::rgb`], for a reversed range.
    pub fn lab(white: [f64; 3], black: [f64; 3], range: [f64; 4]) -> Result<Self> {
        check_points(white, black)?;
        if range[0] > range[1] || range[2] > range[3] {
            return Err(Error::syntax("invalid Lab range"));
        }
        Self::new(Model::Lab { range }, white)
    }

    fn new(model: Model, white: [f64; 3]) -> Result<Self> {
        Ok(Self {
            model,
            white,
            adaptation: adaptation(white)?,
        })
    }

    /// The number of colour components: 1 for CalGray, 3 otherwise.
    pub fn components(&self) -> usize {
        match self.model {
            Model::Gray { .. } => 1,
            Model::Rgb { .. } | Model::Lab { .. } => 3,
        }
    }

    /// Whether this is a Lab space.
    pub fn is_lab(&self) -> bool {
        matches!(self.model, Model::Lab { .. })
    }

    /// The `a*` and `b*` range of a Lab space, `None` for CalGray and CalRGB.
    pub fn lab_range(&self) -> Option<[f64; 4]> {
        match self.model {
            Model::Lab { range } => Some(range),
            Model::Gray { .. } | Model::Rgb { .. } => None,
        }
    }

    /// Converts components, clipped to the space's ranges, to encoded sRGB.
    pub fn convert(&self, components: &[f64]) -> [f64; 3] {
        self.linear(components).map(encode)
    }

    /// The gamut-clipped linear sRGB that [`Self::convert`] encodes.
    pub fn linear(&self, components: &[f64]) -> [f64; 3] {
        let n = |i: usize| components.get(i).copied().unwrap_or(0.0);
        let xyz = match self.model {
            Model::Gray { gamma } => self.white.map(|w| w * n(0).clamp(0.0, 1.0).powf(gamma)),
            Model::Rgb { gamma, matrix } => map(
                matrix,
                [0, 1, 2].map(|i| n(i).clamp(0.0, 1.0).powf(gamma[i])),
            ),
            Model::Lab { range } => lab_to_xyz(
                [
                    n(0).clamp(0.0, 100.0),
                    n(1).clamp(range[0], range[1]),
                    n(2).clamp(range[2], range[3]),
                ],
                self.white,
            ),
        };
        linear_srgb(map(self.adaptation, xyz))
    }
}

/// Validates a white and black point before the family's parameters.
fn check_points(white: [f64; 3], black: [f64; 3]) -> Result<()> {
    if white[0] <= 0.0 || white[1] != 1.0 || white[2] <= 0.0 {
        return Err(Error::syntax("invalid CIE white point"));
    }
    if black.iter().any(|&v| v < 0.0) {
        return Err(Error::syntax("negative CIE black point"));
    }
    if black != [0.0; 3] {
        return Err(Error::unsupported(
            features::BLACK_POINT,
            "nonzero CIE black point is not implemented",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn d50_adaptation_matches_the_runtime_construction() {
        assert_eq!(adaptation(D50).unwrap(), D50_TO_D65);
        // D65 adapts to itself.
        let identity = adaptation(D65).unwrap();
        for (row, values) in identity.iter().enumerate() {
            for (column, &value) in values.iter().enumerate() {
                let expected = if row == column { 1.0 } else { 0.0 };
                assert!((value - expected).abs() < 1e-6, "{row} {column}: {value}");
            }
        }
    }

    #[test]
    fn lab_inverse_is_continuous_at_the_knee() {
        // L* = 8 puts f(Y) exactly at 6/29, where both branches meet.
        let white = [1.0; 3];
        let at = lab_to_xyz([8.0, 0.0, 0.0], white)[1];
        let below = lab_to_xyz([8.0 - 1e-9, 0.0, 0.0], white)[1];
        let above = lab_to_xyz([8.0 + 1e-9, 0.0, 0.0], white)[1];
        assert!((at - 216.0 / 24389.0).abs() < 1e-15);
        assert!(below < at && at < above);
    }

    #[test]
    fn calibrated_spaces_validate_their_parameters() {
        let white = [0.9505, 1.0, 1.089];
        assert!(Calibrated::gray(white, [0.0; 3], 0.0).is_err());
        assert!(Calibrated::lab(white, [0.0; 3], [1.0, 0.0, 0.0, 1.0]).is_err());
        assert!(Calibrated::lab([0.9505, 0.9, 1.089], [0.0; 3], [0.0; 4]).is_err());
        let error = Calibrated::gray(white, [0.1, 0.0, 0.0], 1.0).unwrap_err();
        assert_eq!(error.feature(), Some(features::BLACK_POINT));
        let lab = Calibrated::lab(white, [0.0; 3], [-100.0, 100.0, -100.0, 100.0]).unwrap();
        assert_eq!(lab.components(), 3);
        assert_eq!(lab.lab_range(), Some([-100.0, 100.0, -100.0, 100.0]));
        // Lab white is sRGB white after adaptation.
        let [r, g, b] = lab.convert(&[100.0, 0.0, 0.0]);
        assert!(r > 0.999 && g > 0.999 && b > 0.999, "{r} {g} {b}");
    }
}
