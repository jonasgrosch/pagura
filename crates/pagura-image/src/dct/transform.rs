//! T.81 A.3.3 inverse DCT and PDF Table 13 colour transforms.
use super::{DctImage, frame::Frame};
use pagura_core::{Error, Limits, Result};

// T.81 Figure A.6 diagonal coefficient order, independent of any encoder.
const ORDER: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "rounded samples are clamped to the byte domain"
)]
fn sample(value: f64) -> u8 {
    value.round().clamp(0.0, 255.0) as u8
}

pub(super) fn output(
    frame: &Frame,
    adobe: Option<u8>,
    transform: Option<i64>,
    limits: &Limits,
) -> Result<DctImage> {
    let n = u8::try_from(frame.components.len())
        .map_err(|_| Error::syntax("invalid JPEG component count"))?;
    let count = crate::parameters::budget(frame.width, frame.height, n, 8, limits)?;
    let mut basis = [[0.0; 8]; 8];
    for (x, row) in basis.iter_mut().enumerate() {
        for (u, value) in row.iter_mut().enumerate() {
            *value = if u == 0 { 0.5f64.sqrt() } else { 1.0 }
                * (((2 * x + 1) * u) as f64 * std::f64::consts::PI / 16.0).cos()
                * 0.5;
        }
    }
    let mut planes = Vec::with_capacity(usize::from(n));
    for c in &frame.components {
        if c.approximation[0] < 0 || (!frame.progressive && c.approximation.iter().any(|&v| v < 0))
        {
            return Err(Error::syntax("JPEG component has no complete scan"));
        }
        let quant = c.quant.ok_or(Error::syntax("missing JPEG quantizer"))?;
        let mut plane = vec![0; c.stride * c.rows * 64];
        for (index, coefficients) in c.blocks.iter().enumerate() {
            let mut natural = [0.0; 64];
            for ((&coefficient, &q), &position) in coefficients.iter().zip(&quant).zip(&ORDER) {
                natural[position] = f64::from(coefficient) * f64::from(q);
            }
            let mut horizontal = [[0.0; 8]; 8];
            for (v, row) in horizontal.iter_mut().enumerate() {
                for (x, value) in row.iter_mut().enumerate() {
                    *value = natural[v * 8..v * 8 + 8]
                        .iter()
                        .zip(&basis[x])
                        .map(|(a, b)| a * b)
                        .sum();
                }
            }
            for (y, row) in basis.iter().enumerate() {
                for x in 0..8 {
                    let value = horizontal
                        .iter()
                        .zip(row)
                        .map(|(v, b)| v[x] * b)
                        .sum::<f64>();
                    let destination =
                        (index / c.stride * 8 + y) * c.stride * 8 + index % c.stride * 8 + x;
                    plane[destination] = sample(value + 128.0);
                }
            }
        }
        planes.push(plane);
    }
    // Table 13 ignores the parameter for one component and whenever an
    // Adobe marker specifies the transform.
    if adobe.is_none() && n > 1 && transform.is_some_and(|v| !matches!(v, 0 | 1)) {
        return Err(Error::syntax("invalid DCT ColorTransform"));
    }
    let convert = match adobe {
        Some(0) => false,
        Some(1 | 2) => true,
        _ => transform.map_or(n == 3, |v| v == 1),
    };
    if adobe == Some(2) && n != 4 {
        return Err(Error::syntax("Adobe YCCK marker requires four components"));
    }
    let mut samples = Vec::with_capacity(count);
    for y in 0..frame.height as usize {
        for x in 0..frame.width as usize {
            let mut pixel = [0; 4];
            for (i, c) in frame.components.iter().enumerate() {
                let sx = x * c.h / frame.max_h;
                let sy = y * c.v / frame.max_v;
                pixel[i] = planes[i][sy * c.stride * 8 + sx];
            }
            if convert && n >= 3 {
                let (y, cb, cr) = (
                    f64::from(pixel[0]),
                    f64::from(pixel[1]) - 128.0,
                    f64::from(pixel[2]) - 128.0,
                );
                pixel[0] = sample(y + 1.402 * cr);
                pixel[1] = sample(y - 0.344_136 * cb - 0.714_136 * cr);
                pixel[2] = sample(y + 1.772 * cb);
                if n == 4 {
                    for c in &mut pixel[..3] {
                        *c = 255 - *c;
                    }
                }
            }
            samples.extend_from_slice(&pixel[..usize::from(n)]);
        }
    }
    Ok(DctImage {
        width: frame.width,
        height: frame.height,
        components: n,
        samples,
    })
}
