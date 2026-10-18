//! CIE colorimetry shared by calibrated PDF spaces and ICC profiles.

use pagura_core::{Error, Result};

/// The inverse of the CIELAB companding function f (CIE 15).
pub fn lab_inverse(v: f64) -> f64 {
    if v >= 6.0 / 29.0 {
        v * v * v
    } else {
        (108.0 / 841.0) * (v - 4.0 / 29.0)
    }
}
/// Multiplies a 3×3 matrix by a column vector.
pub fn map(matrix: [[f64; 3]; 3], value: [f64; 3]) -> [f64; 3] {
    matrix.map(|row| row.into_iter().zip(value).map(|(a, b)| a * b).sum())
}
/// Bradford chromatic adaptation from `white` to the D65 sRGB white.
///
/// # Errors
/// Returns `Limit` for a white point whose cone response is singular.
pub fn adaptation(white: [f64; 3]) -> Result<[[f64; 3]; 3]> {
    let bradford = [
        [0.8951, 0.2664, -0.1614],
        [-0.7502, 1.7135, 0.0367],
        [0.0389, -0.0685, 1.0296],
    ];
    let inverse = [
        [0.986_992_9, -0.147_054_3, 0.159_962_7],
        [0.432_305_3, 0.518_360_3, 0.049_291_2],
        [-0.008_528_7, 0.040_042_8, 0.968_486_7],
    ];
    let source = map(bradford, white);
    let target = map(bradford, [0.95047, 1.0, 1.08883]);
    if source.iter().any(|v| v.abs() < 1e-12) {
        return Err(Error::limit("CIE white adaptation is singular"));
    }
    let ratio = [
        target[0] / source[0],
        target[1] / source[1],
        target[2] / source[2],
    ];
    let mut result = [[0.0; 3]; 3];
    for (row, output) in result.iter_mut().enumerate() {
        for (column, out) in output.iter_mut().enumerate() {
            *out = (0..3)
                .map(|k| inverse[row][k] * ratio[k] * bradford[k][column])
                .sum();
        }
    }
    Ok(result)
}

/// Converts D65-relative XYZ to gamut-clipped, encoded sRGB (IEC 61966-2-1).
pub fn xyz_to_srgb(xyz: [f64; 3]) -> [f64; 3] {
    let rgb = map(
        [
            [3.240_454_2, -1.537_138_5, -0.498_531_4],
            [-0.969_266, 1.876_010_8, 0.041_556],
            [0.055_643_4, -0.204_025_9, 1.057_225_2],
        ],
        xyz,
    );
    rgb.map(|v| {
        let v = v.clamp(0.0, 1.0);
        if v <= 0.003_130_8 {
            12.92 * v
        } else {
            1.055 * v.powf(1.0 / 2.4) - 0.055
        }
    })
}
