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

trait Sample: Copy + Default {
    fn from_idct(value: f64) -> Self;
    fn fused(_frame: &Frame, _planes: &[Vec<Self>], _samples: &mut [Self]) -> bool {
        false
    }
    fn finish(samples: Vec<Self>, components: usize, convert: bool) -> Vec<u8>;
}

impl Sample for u8 {
    fn fused(frame: &Frame, planes: &[Vec<Self>], samples: &mut [Self]) -> bool {
        fused_rgb(frame, planes, samples)
    }
    fn from_idct(value: f64) -> Self {
        sample(value + 128.0)
    }

    fn finish(mut samples: Vec<Self>, components: usize, convert: bool) -> Vec<u8> {
        if convert && components >= 3 {
            for pixel in samples.chunks_exact_mut(components) {
                let rgb = ycbcr(
                    f64::from(pixel[0]),
                    f64::from(pixel[1]) - 128.0,
                    f64::from(pixel[2]) - 128.0,
                );
                for (out, value) in pixel[..3].iter_mut().zip(rgb) {
                    *out = sample(value);
                    if components == 4 {
                        *out = 255 - *out;
                    }
                }
            }
        }
        samples
    }
}

impl Sample for u16 {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "T.81 A.3.1 native samples are rounded and clamped to 0..4095"
    )]
    fn from_idct(value: f64) -> Self {
        (value + 2048.0).round().clamp(0.0, 4095.0) as Self
    }

    fn finish(samples: Vec<Self>, components: usize, convert: bool) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(samples.len());
        for pixel in samples.chunks_exact(components) {
            if convert && components >= 3 {
                let rgb = ycbcr(
                    f64::from(pixel[0]),
                    f64::from(pixel[1]) - 2048.0,
                    f64::from(pixel[2]) - 2048.0,
                );
                for value in rgb {
                    let native = value.clamp(0.0, 4095.0);
                    let native = if components == 4 {
                        4095.0 - native
                    } else {
                        native
                    };
                    // Keep fractional colour results until the final byte rounding.
                    bytes.push(sample(native * 255.0 / 4095.0));
                }
                bytes.extend(pixel[3..].iter().map(|&v| reduce(v)));
            } else {
                bytes.extend(pixel.iter().map(|&v| reduce(v)));
            }
        }
        bytes
    }
}

/// PDF 7.4.8 specifies bytes, but not twelve-bit normalization. Preserve
/// endpoints and round once; the odd denominator precludes half-level ties.
fn reduce(value: u16) -> u8 {
    u8::try_from((u32::from(value) * 255 + 2047) / 4095).unwrap_or(255)
}

fn ycbcr(y: f64, cb: f64, cr: f64) -> [f64; 3] {
    [
        y + 1.402 * cr,
        y - 0.344_136 * cb - 0.714_136 * cr,
        y + 1.772 * cb,
    ]
}

pub(super) fn output(
    frame: &Frame,
    adobe: Option<u8>,
    transform: Option<i64>,
    limits: &Limits,
) -> Result<DctImage> {
    if frame.precision == 8 {
        output_native::<u8>(frame, adobe, transform, limits)
    } else {
        output_native::<u16>(frame, adobe, transform, limits)
    }
}

fn output_native<T: Sample>(
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
        let mut plane = vec![T::default(); c.stride * c.rows * 64];
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
                let block = idct.block::<T>(&natural);
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
    let mut samples = vec![T::default(); count];
    if convert && T::fused(frame, &planes, &mut samples) {
        return Ok(DctImage {
            width: frame.width,
            height: frame.height,
            components: n,
            samples: T::finish(samples, usize::from(n), false),
        });
    }
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
    Ok(DctImage {
        width: frame.width,
        height: frame.height,
        components: n,
        samples: T::finish(samples, components, convert),
    })
}

/// Expand matching chroma planes directly into RGB. Chroma contributions are
/// evaluated once per source run; each output keeps the byte-domain rounding
/// and the original multiplication/subtraction order (7.4.8, Table 13).
fn fused_rgb(frame: &Frame, planes: &[Vec<u8>], samples: &mut [u8]) -> bool {
    let [y, cb, cr] = frame.components.as_slice() else {
        return false;
    };
    if y.h != frame.max_h
        || y.v != frame.max_v
        || cb.h != cr.h
        || cb.v != cr.v
        || !frame.max_h.is_multiple_of(cb.h)
    {
        return false;
    }
    let repeat = frame.max_h / cb.h;
    let width = frame.width as usize;
    let pitch = y.stride * 8;
    let chroma_pitch = cb.stride * 8;
    for (line, output) in samples.chunks_exact_mut(width * 3).enumerate() {
        let luminance = &planes[0][line * pitch..line * pitch + width];
        let start = line * cb.v / frame.max_v * chroma_pitch;
        let blue = &planes[1][start..start + chroma_pitch];
        let red = &planes[2][start..start + chroma_pitch];
        for (((run, yy), &cb), &cr) in output
            .chunks_mut(repeat * 3)
            .zip(luminance.chunks(repeat))
            .zip(blue)
            .zip(red)
        {
            let cb = f64::from(cb) - 128.0;
            let cr = f64::from(cr) - 128.0;
            let (red, green_cb, green_cr, blue) =
                (1.402 * cr, 0.344_136 * cb, 0.714_136 * cr, 1.772 * cb);
            for (pixel, &y) in run.as_chunks_mut::<3>().0.iter_mut().zip(yy) {
                let y = f64::from(y);
                *pixel = [
                    sample(y + red),
                    sample(y - green_cb - green_cr),
                    sample(y + blue),
                ];
            }
        }
    }
    true
}

/// T.81 A.1.1 sampling factors: each source sample occupies a contiguous run.
/// Integer ratios use replication; a remainder handles ratios such as 3:4
/// without division in the pixel loop. The last run can be cropped by the frame.
fn expand<T: Copy>(source: &[T], destination: &mut [T], components: usize, h: usize, max_h: usize) {
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
    fn fused_chroma_runs_keep_byte_rounding_and_cropped_coordinates() {
        let limits = pagura_core::Limits::default();
        for yh in 1..=4 {
            for yv in 1..=4 {
                for ch in 1..=yh {
                    for cv in 1..=yv {
                        let frame = super::Frame::parse(
                            &[
                                8,
                                0,
                                11,
                                0,
                                9,
                                3,
                                1,
                                yh * 16 + yv,
                                0,
                                2,
                                ch * 16 + cv,
                                0,
                                3,
                                ch * 16 + cv,
                                0,
                            ],
                            0xc0,
                            &limits,
                        )
                        .unwrap();
                        let planes: Vec<Vec<_>> = frame
                            .components
                            .iter()
                            .enumerate()
                            .map(|(i, c)| {
                                (0..c.stride * c.rows * 64)
                                    .map(|p| u8::try_from((p * 37 + i * 71) % 256).unwrap())
                                    .collect()
                            })
                            .collect();
                        let mut actual = vec![0; 9 * 11 * 3];
                        assert_eq!(super::fused_rgb(&frame, &planes, &mut actual), yh % ch == 0);
                        if yh % ch != 0 {
                            continue;
                        }
                        for y in 0..11 {
                            for x in 0..9 {
                                let source: Vec<_> = frame
                                    .components
                                    .iter()
                                    .zip(&planes)
                                    .map(|(c, p)| {
                                        f64::from(
                                            p[(y * c.v / frame.max_v) * (c.stride * 8)
                                                + x * c.h / frame.max_h],
                                        )
                                    })
                                    .collect();
                                let (yy, cb, cr) =
                                    (source[0], source[1] - 128.0, source[2] - 128.0);
                                let expected = [
                                    super::sample(yy + 1.402 * cr),
                                    super::sample(yy - 0.344_136 * cb - 0.714_136 * cr),
                                    super::sample(yy + 1.772 * cb),
                                ];
                                assert_eq!(
                                    &actual[(y * 9 + x) * 3..(y * 9 + x + 1) * 3],
                                    &expected
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn native_reconstruction_and_reduction_preserve_the_full_range() {
        use super::Sample;
        for native in 0..=4095u16 {
            assert_eq!(u16::from_idct(f64::from(native) - 2048.0), native);
            let exact = f64::from(native) * 255.0 / 4095.0;
            assert!((f64::from(super::reduce(native)) - exact).abs() < 0.5);
        }
        assert_eq!(u16::from_idct(-3000.0), 0);
        assert_eq!(u16::from_idct(3000.0), 4095);
    }

    #[test]
    fn eight_bit_colour_transform_keeps_byte_rounding() {
        use super::Sample;
        // Independent byte-domain formulas also pin inversion after rounding.
        for y in 0..=255u8 {
            for chroma in 0..=255u8 {
                for components in [3, 4] {
                    let pixel = [y, chroma, 255 - chroma, 73];
                    let (cb, cr) = (f64::from(chroma) - 128.0, 127.0 - f64::from(chroma));
                    let rgb = [
                        f64::from(y) + 1.402 * cr,
                        f64::from(y) - 0.344_136 * cb - 0.714_136 * cr,
                        f64::from(y) + 1.772 * cb,
                    ];
                    let mut expected = pixel[..components].to_vec();
                    for (out, value) in expected[..3].iter_mut().zip(rgb) {
                        let byte = super::sample(value);
                        *out = if components == 4 { 255 - byte } else { byte };
                    }
                    assert_eq!(
                        u8::finish(pixel[..components].to_vec(), components, true),
                        expected
                    );
                }
            }
        }
    }

    #[test]
    fn native_colour_rounding_matches_exact_integer_rationals() {
        use super::Sample;
        let mut state = 0x1234_5678u32;
        for _ in 0..100_000 {
            let pixel = std::array::from_fn::<_, 4, _>(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                u16::try_from(state >> 20).unwrap()
            });
            let (y, cb, cr) = (
                i64::from(pixel[0]) * 1_000_000,
                i64::from(pixel[1]) - 2048,
                i64::from(pixel[2]) - 2048,
            );
            let rgb = [
                y + 1_402_000 * cr,
                y - 344_136 * cb - 714_136 * cr,
                y + 1_772_000 * cb,
            ];
            let denominator = 4095 * 1_000_000i64;
            for components in [3, 4] {
                let bytes = u16::finish(pixel[..components].to_vec(), components, true);
                for (&actual, exact) in bytes.iter().zip(rgb) {
                    let exact = exact.clamp(0, denominator);
                    let exact = if components == 4 {
                        denominator - exact
                    } else {
                        exact
                    };
                    let error = (i64::from(actual) * denominator - exact * 255).abs();
                    assert!(
                        2 * error <= denominator,
                        "{pixel:?}: {actual} vs {exact}/{denominator}"
                    );
                }
                if components == 4 {
                    assert_eq!(bytes[3], super::reduce(pixel[3]));
                }
            }
        }
    }

    #[test]
    fn twelve_bit_expansion_retains_low_bits() {
        let source = [1u16, 2047, 2048, 4094, 4095];
        let mut expanded = [0; 10];
        expand(&source, &mut expanded, 1, 1, 2);
        assert_eq!(
            expanded,
            [1, 1, 2047, 2047, 2048, 2048, 4094, 4094, 4095, 4095]
        );
    }

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
