//! T.81 A.3.3 inverse DCT and PDF Table 13 colour transforms.
mod idct;

use super::{DctImage, frame::Frame};
use idct::Idct;
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
    let idct = Idct::new();
    let mut planes = Vec::with_capacity(usize::from(n));
    for c in &frame.components {
        if c.approximation[0] < 0 || (!frame.progressive && c.approximation.iter().any(|&v| v < 0))
        {
            return Err(Error::syntax("JPEG component has no complete scan"));
        }
        let quant = c.quant.ok_or(Error::syntax("missing JPEG quantizer"))?;
        let mut plane = vec![0; c.stride * c.rows * 64];
        let pitch = c.stride * 8;
        for (blocks, rows) in c
            .blocks
            .chunks_exact(c.stride)
            .zip(plane.chunks_exact_mut(pitch * 8))
        {
            for (column, coefficients) in blocks.iter().enumerate() {
                let mut natural = [0.0; 64];
                for ((&coefficient, &q), &position) in coefficients.iter().zip(&quant).zip(&ORDER) {
                    natural[position] = f64::from(coefficient) * f64::from(q);
                }
                let block = idct.block(&natural);
                for (row, samples) in rows.chunks_exact_mut(pitch).zip(&block) {
                    row[column * 8..column * 8 + 8].copy_from_slice(samples);
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
    let mut samples = vec![0; count];
    let components = usize::from(n);
    let width = frame.width as usize;
    for (i, c) in frame.components.iter().enumerate() {
        let pitch = c.stride * 8;
        for (y, row) in samples.chunks_exact_mut(width * components).enumerate() {
            let sy = y * c.v / frame.max_v;
            let source = &planes[i][sy * pitch..(sy + 1) * pitch];
            expand(source, &mut row[i..], components, c.h, frame.max_h);
        }
    }
    if convert && n >= 3 {
        for pixel in samples.chunks_exact_mut(components) {
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
    }
    Ok(DctImage {
        width: frame.width,
        height: frame.height,
        components: n,
        samples,
    })
}

/// T.81 A.1.1 sampling factors: each source sample occupies a contiguous run.
/// Integer ratios use replication; a remainder handles ratios such as 3:4
/// without division in the pixel loop. The last run can be cropped by the frame.
fn expand(source: &[u8], destination: &mut [u8], components: usize, h: usize, max_h: usize) {
    if max_h.is_multiple_of(h) {
        let repeat = max_h / h;
        for (&value, run) in source
            .iter()
            .zip(destination.chunks_mut(repeat * components))
        {
            for out in run.iter_mut().step_by(components) {
                *out = value;
            }
        }
    } else {
        let (mut index, mut remainder) = (0, 0);
        for out in destination.iter_mut().step_by(components) {
            *out = source[index];
            remainder += h;
            if remainder >= max_h {
                remainder -= max_h;
                index += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::expand;

    #[test]
    fn block_rows_and_padding_preserve_component_coordinates() {
        let limits = pagura_core::Limits::default();
        let mut frame = super::Frame::parse(
            &[8, 0, 9, 0, 29, 3, 1, 0x31, 0, 2, 0x21, 0, 3, 0x11, 0],
            0xc0,
            &limits,
        )
        .unwrap();
        for (i, component) in frame.components.iter_mut().enumerate() {
            component.quant = Some([1; 64]);
            component.approximation = [0; 64];
            for (index, block) in component.blocks.iter_mut().enumerate() {
                block[0] = i32::try_from(8 * (16 * i + index)).unwrap();
            }
        }
        let image = super::output(&frame, None, Some(0), &limits).unwrap();
        assert_eq!(image.samples.len(), 29 * 9 * 3);
        for (y, row) in image.samples.as_chunks::<87>().0.iter().enumerate() {
            for (x, pixel) in row.as_chunks::<3>().0.iter().enumerate() {
                for (i, (&sample, component)) in pixel.iter().zip(&frame.components).enumerate() {
                    let block = (y / 8) * component.stride + (x * component.h / frame.max_h) / 8;
                    assert_eq!(usize::from(sample), 128 + 16 * i + block);
                }
            }
        }
    }

    #[test]
    fn cropped_replication_matches_sampling_factors() {
        let source: Vec<u8> = (0..64).collect();
        for max_h in 1..=4 {
            for h in 1..=max_h {
                for width in 1..=35 {
                    for components in [1, 3, 4] {
                        for component in 0..components {
                            let mut out = vec![255; width * components];
                            expand(&source, &mut out[component..], components, h, max_h);
                            for (x, pixel) in out.chunks_exact(components).enumerate() {
                                for (i, &value) in pixel.iter().enumerate() {
                                    assert_eq!(
                                        value,
                                        if i == component {
                                            source[x * h / max_h]
                                        } else {
                                            255
                                        }
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
